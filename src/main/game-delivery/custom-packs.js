"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const fileOps = require("./file-ops");
const { writeJsonAtomic, readJsonSafe } = require("./atomic");
const { DeliveryError } = require("./errors");

// Моды игр (модели, HUD, звуки) с переключателем в персонализации. Файлы мода
// приходят патчем игры в <мод>/hlsr-packs/<id> и лежат там, движок их не видит.
// Включение копирует их туда, где движок ищет файлы раньше оригиналов, сами
// оригиналы не меняются; выключение удаляет копии.
//
// HL1 (GoldSrc): копии ложатся в папку мода valve_WON, её движок читает раньше
// valve. Если в valve_WON на этом месте уже был чужой файл, он уходит в
// .hlsr/custom-packs/backup и потом возвращается.
// Ghosting (Source): мод целиком ложится папкой в ghostingmod/custom/<id>.
// gameinfo.txt монтирует custom/* первым, раньше файлов мода и его VPK.
//
// Перед любой правкой файлов в state.json пишется журнал (pending). Сбой
// посередине доделывает следующий вызов: включение откатывается, выключение
// доводится до конца. Оба случая — одно и то же «вернуть как было» (undo).

const GAMES = {
  "hl1-won": {
    sourceDir: "Half-Life/valve_WON/hlsr-packs",
    targetDir: () => "Half-Life/valve_WON",
    pruneRoot: "Half-Life/valve_WON",
    baseDir: "Half-Life/valve",
  },
  "ghosting-renewed": {
    sourceDir: "ghostingmod/hlsr-packs",
    targetDir: (pack) => `ghostingmod/custom/${pack.id}`,
    pruneRoot: "ghostingmod",
    baseDir: null,
  },
};
const STATE_DIR = ".hlsr/custom-packs";
const TEMP_SUFFIX = ".hlsr-tmp";
const MAX_SOUND_BYTES = 2 * 1024 * 1024;

const under = (dir, names) => names.map((name) => `${dir}/${name}`);
const each = (stems, extensions) => stems.reduce((all, stem) => all.concat(extensions.map((ext) => stem + ext)), []);

// Пути внутри мода. Имена — как в игре (WON — строчные; Source регистр не различает).
const PACKS = [
  {
    id: "arianon-viewmodels",
    game: "hl1-won",
    kind: "models",
    author: "Arianon",
    files: under("models", [
      "v_357.mdl", "v_9mmar.mdl", "v_9mmhandgun.mdl", "v_crossbow.mdl", "v_crowbar.mdl",
      "v_egon.mdl", "v_gauss.mdl", "v_grenade.mdl", "v_hgun.mdl", "v_rpg.mdl", "v_satchel.mdl",
      "v_satchel_radio.mdl", "v_shotgun.mdl", "v_squeak.mdl", "v_tripmine.mdl",
    ]),
  },
  {
    id: "item-sprites",
    game: "hl1-won",
    kind: "sprites",
    author: null,
    files: under("models", [
      "w_357.mdl", "w_357ammobox.mdl", "w_9mmar.mdl", "w_9mmarclip.mdl", "w_9mmclip.mdl",
      "w_9mmhandgun.mdl", "w_argrenade.mdl", "w_battery.mdl", "w_crossbow.mdl", "w_crossbow_clip.mdl",
      "w_egon.mdl", "w_gauss.mdl", "w_gaussammo.mdl", "w_grenade.mdl", "w_hgun.mdl", "w_longjump.mdl",
      "w_medkit.mdl", "w_rpg.mdl", "w_rpgammo.mdl", "w_satchel.mdl", "w_shotbox.mdl", "w_shotgun.mdl",
      "w_sqknest.mdl",
    ]),
  },
  {
    id: "oxcjae-soundpack-lite",
    game: "hl1-won",
    kind: "sounds",
    author: "oxcjae",
    files: [].concat(
      under("sound/barney", [
        "ba_die3.wav", "ba_dontmake.wav", "ba_endline.wav", "ba_iwish.wav", "ba_somuch.wav",
        "ba_stepoff.wav", "ba_tomb.wav", "ba_uwish.wav", "c1a0_ba_hevyes.wav", "c1a4_ba_octo3.wav",
        "c2a2_ba_launch.wav", "c2a3_ba_assn.wav", "c2a4_ba_5tau.wav", "c2a4_ba_alive.wav",
      ]),
      under("sound/hgrunt", [
        "c2a3_ambush_vox.wav", "c2a3_hg_1drag.wav", "c2a3_hg_2drag.wav", "c2a3_hg_3drag.wav",
        "c2a3_hg_4drag.wav", "c2a3_hg_5drag.wav", "c2a3_hg_laugh.wav", "c2a5_hg_abandon.wav",
        "c2a5_hg_lebuz.wav", "hg_civvies.wav",
      ]),
      under("sound/scientist", [
        "afellowsci.wav", "ahfreeman.wav", "c1a0_sci_crit1a.wav", "c1a0_sci_ctrl1a.wav",
        "c1a0_sci_ctrl2a.wav", "c1a0_sci_dis1a.wav", "c1a0_sci_dis9a.wav", "c1a0_sci_disa.wav",
        "c1a0_sci_lock4a.wav", "c1a0_sci_lock6a.wav", "c1a2_sci_dangling.wav", "c1a3_sci_silo1a.wav",
        "c1a3_sci_silo2a.wav", "c1a4_sci_gener.wav", "c2a3_sci_track.wav", "c2a4_sci_letout.wav",
        "c2a5_sci_boobie.wav", "c3a2_sci_fool.wav", "canttakemore.wav", "freeman.wav",
        "freemanalive.wav", "goodtoseeyou.wav", "greetings.wav", "greetings2.wav", "heal1.wav",
        "heal2.wav", "heal3.wav", "heal4.wav", "heal5.wav", "hello.wav", "hello2.wav",
        "hellofreeman.wav", "hellofromlab.wav", "hellothere.wav", "holdstill.wav", "inmesstoo.wav",
        "iwounded.wav", "iwounded2.wav", "iwoundedbad.wav", "letstrythis.wav", "newhevsuit.wav",
        "sci_pain1.wav", "sci_pain2.wav", "sci_pain3.wav", "sci_pain4.wav", "sci_pain5.wav",
        "scream01.wav", "scream3.wav", "scream4.wav", "youlookbad.wav", "youneedmedic.wav",
        "youwounded.wav",
      ])
    ),
  },
  // HUD из Synergy с блоками Ghosting (таймер, зеркало, «забег завершён»),
  // дописанными при подготовке выпуска: без них эти элементы пропали бы.
  {
    id: "condensed-hud",
    game: "ghosting-renewed",
    kind: "hud",
    author: null,
    files: [].concat(
      under("materials/vgui/hud", each(
        ["icon_acid", "icon_blank", "icon_blast", "icon_energy", "icon_heat", "icon_oxygen", "icon_poison", "icon_radiation", "icon_shock"],
        [".vmt", ".vtf"]
      )),
      under("resource", [
        "clientscheme.res", "valve_english.txt", "valve_russian.txt", "synergy_english.txt", "Hype font.txt", "LICENSE_OFL.txt",
        "din1451alt.ttf", "din1451alt2.ttf", "FacileSans.ttf", "halflife2.ttf", "hl2crosshairs.ttf",
        "HL2EP2.ttf", "HL2MMod.ttf", "HL2MP.ttf", "HYPE.ttf", "K12HL2.ttf", "lostcoast.ttf",
        "nokia-pure-text.ttf", "RealBeta's Weapon Icons.ttf", "SynFont.ttf",
      ]),
      under("resource", each(["NotoSans-"], ["Bold.ttf", "BoldItalic.ttf", "Italic.ttf", "Regular.ttf"])),
      under("resource", each(["Roboto-"], [
        "Black.ttf", "BlackItalic.ttf", "Bold.ttf", "BoldItalic.ttf", "Italic.ttf", "Light.ttf",
        "LightItalic.ttf", "Medium.ttf", "MediumItalic.ttf", "Regular.ttf", "Thin.ttf", "ThinItalic.ttf",
      ])),
      under("resource", each(["RobotoCondensed-"], [
        "Bold.ttf", "BoldItalic.ttf", "Italic.ttf", "Light.ttf", "LightItalic.ttf", "Regular.ttf",
      ])),
      under("scripts", ["hudanimations.txt", "hudanimations_manifest.txt", "hudanimations_syn.txt", "hudlayout.res"])
    ),
  },
  // Только модель в руках: мировая w_crowbar с другими размерами меняла бы
  // зону подбора и физику лежащего лома.
  {
    id: "m9-bayonet",
    game: "ghosting-renewed",
    kind: "models",
    author: null,
    files: [].concat(
      under("models/weapons", each(["v_crowbar"], [".mdl", ".ani", ".vvd", ".dx80.vtx", ".dx90.vtx", ".sw.vtx"])),
      under("materials/models/csgo_knife", [
        "knife_m9_bay.vmt", "knife_m9_bay.vtf", "knife_m9_bay_ao.vtf", "knife_m9_bay_exponent.vtf",
      ])
    ),
  },
  // Перекраски Eold (GameBanana 490327, 501191, 551903): цветной вариант из архива, как на его
  // снимках. Только текстура: .vmt в архивах те же, что в hl2_misc_dir.vpk, их не берём.
  {
    id: "signed-smg",
    game: "ghosting-renewed",
    kind: "skins",
    author: "Eold",
    files: ["materials/models/weapons/v_smg1/v_smg1_sheet.vtf"],
  },
  {
    id: "signed-shotgun",
    game: "ghosting-renewed",
    kind: "skins",
    author: "Eold",
    files: ["materials/models/weapons/v_shotgun/vshotgun_albedo.vtf"],
  },
  {
    id: "signed-rpg",
    game: "ghosting-renewed",
    kind: "skins",
    author: "Eold",
    files: ["materials/models/weapons/v_rocket_launcher/rocket launcher sheet.vtf"],
  },
];

const BY_ID = new Map(PACKS.map((pack) => [pack.id, pack]));

// Куда ложатся копии (пути от папки игры): для проверки файлов это слой
// кастомизации (mutable-paths.js).
function targets() {
  return PACKS.reduce((all, pack) => all.concat(pack.files.map((file) => `${GAMES[pack.game].targetDir(pack)}/${file}`)), []);
}

function packOf(id) {
  const pack = BY_ID.get(String(id));
  if (!pack) throw new DeliveryError("CUSTOM_PACK_UNKNOWN", `Неизвестный пак: ${String(id)}`);
  return pack;
}

function hasPacks(gameId) {
  return PACKS.some((pack) => pack.game === gameId);
}

function sha256Of(file) {
  try {
    return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  } catch (error) {
    if (error.code === "ENOENT" || error.code === "EISDIR") return null;
    throw error;
  }
}

function sizeOf(file) {
  try {
    const stats = fs.statSync(file);
    return stats.isFile() ? stats.size : -1;
  } catch (error) {
    return -1;
  }
}

function layout(installPath, pack) {
  const game = GAMES[pack.game];
  const stateRoot = fileOps.resolveInside(installPath, STATE_DIR);
  const backupRoot = path.join(stateRoot, "backup");
  const targetDir = game.targetDir(pack);
  return {
    stateRoot,
    stateFile: path.join(stateRoot, "state.json"),
    pruneRoot: fileOps.resolveInside(installPath, game.pruneRoot),
    source: (file) => fileOps.resolveInside(installPath, `${game.sourceDir}/${pack.id}/${file}`),
    target: (file) => fileOps.resolveInside(installPath, `${targetDir}/${file}`),
    backup: (file) => fileOps.resolveInside(backupRoot, `${targetDir}/${file}`),
  };
}

function readState(file) {
  const state = readJsonSafe(file);
  return {
    packs: state && state.packs && typeof state.packs === "object" ? state.packs : {},
    pending: state && state.pending && typeof state.pending === "object" ? state.pending : null,
  };
}

const stateFileOf = (installPath) => path.join(fileOps.resolveInside(installPath, STATE_DIR), "state.json");

// Копия через временный файл рядом: на месте цели всегда либо прежний файл,
// либо новый целиком, недописанного не бывает.
function copyAtomic(source, target, relative) {
  const temp = target + TEMP_SUFFIX;
  fileOps.copyFileSync(source, temp, relative);
  fileOps.makeWritableSync(temp);
  fileOps.makeWritableSync(target);
  try {
    fs.renameSync(temp, target);
  } catch (error) {
    fs.rmSync(temp, { force: true });
    throw fileOps.wrapLockError(error, relative) || error;
  }
}

// Возвращает всё, как было до включения. Трогает только файлы, которые
// сейчас совпадают с копией пака: изменённый после включения файл уже чужой.
function undo(where, files, hooks = {}) {
  for (const file of Object.keys(files)) {
    const entry = files[file] || {};
    const target = where.target(file);
    const backup = where.backup(file);
    fileOps.removeFileSync(target + TEMP_SUFFIX, file);
    fileOps.removeFileSync(backup + TEMP_SUFFIX, file);

    if (sha256Of(target) === entry.sha256) {
      if (!entry.backup) fileOps.removeFileSync(target, file);
      else if (sizeOf(backup) >= 0) copyAtomic(backup, target, file);
      // Копии чужого файла нет (сбой до неё) — значит, он не тронут.
    }
    fileOps.removeFileSync(backup, file);
    fileOps.pruneEmptyDirs(where.pruneRoot, path.dirname(target));
    fileOps.pruneEmptyDirs(where.stateRoot, path.dirname(backup));
    if (hooks.afterFile) hooks.afterFile(file);
  }
}

// Копии на месте и совпадают с файлами пака, а записи о включении нет: состояние
// потеряно (переустановка переносит ghostingmod/custom, но не .hlsr). Такой пак
// включён; выключение уберёт копии, резервных копий у него нет.
function adoptedRecord(pack, where) {
  for (const file of pack.files) {
    const size = sizeOf(where.source(file));
    if (size < 0 || sizeOf(where.target(file)) !== size) return null;
  }
  const files = {};
  for (const file of pack.files) {
    const sha256 = sha256Of(where.source(file));
    if (sha256Of(where.target(file)) !== sha256) return null;
    files[file] = { sha256, backup: false };
  }
  return { files };
}

// Доделывает операцию, прерванную сбоем. true — было что доделать.
function recover(installPath, options = {}) {
  const logger = options.logger || { info() {}, warn() {} };
  const stateFile = stateFileOf(installPath);
  const state = readState(stateFile);
  if (!state.pending) return false;

  const pending = state.pending;
  const pack = BY_ID.get(pending.id);
  if (pack) undo(layout(installPath, pack), pending.files || {});
  delete state.packs[pending.id];
  state.pending = null;
  writeJsonAtomic(stateFile, state);
  logger.warn(`[custom-packs] пак ${pending.id}: прерванное ${pending.op === "enable" ? "включение откачено" : "выключение доведено"}`);
  return true;
}

function isAvailable(installPath, pack) {
  if (!installPath) return false;
  const where = layout(installPath, pack);
  return pack.files.every((file) => sizeOf(where.source(file)) >= 0);
}

// Паки игры. options.recover — доделать прерванное (только когда игра не запущена).
function list(installPath, gameId, options = {}) {
  if (installPath && options.recover) recover(installPath, options);
  const state = installPath ? readState(stateFileOf(installPath)) : { packs: {} };
  return PACKS.filter((pack) => pack.game === gameId).map((pack) => {
    const available = isAvailable(installPath, pack);
    return {
      id: pack.id,
      kind: pack.kind,
      author: pack.author,
      files: pack.files.slice(),
      available,
      enabled: !!state.packs[pack.id] || (available && !!adoptedRecord(pack, layout(installPath, pack))),
    };
  });
}

function enable(pack, state, where, hooks) {
  const files = {};
  for (const file of pack.files)
    files[file] = { sha256: sha256Of(where.source(file)), backup: sizeOf(where.target(file)) >= 0 };

  state.pending = { id: pack.id, op: "enable", files };
  writeJsonAtomic(where.stateFile, state);

  try {
    for (const file of pack.files) {
      if (files[file].backup) copyAtomic(where.target(file), where.backup(file), file);
      copyAtomic(where.source(file), where.target(file), file);
      if (hooks.afterFile) hooks.afterFile(file);
    }
  } catch (error) {
    undo(where, files);
    state.pending = null;
    writeJsonAtomic(where.stateFile, state);
    throw error;
  }

  state.packs[pack.id] = { files };
  state.pending = null;
  writeJsonAtomic(where.stateFile, state);
}

function disable(pack, state, where, record, hooks) {
  const files = record.files || {};
  state.pending = { id: pack.id, op: "disable", files };
  writeJsonAtomic(where.stateFile, state);

  undo(where, files, hooks);
  delete state.packs[pack.id];
  state.pending = null;
  writeJsonAtomic(where.stateFile, state);
}

// Игра в это время не должна быть запущена (проверяет вызывающий).
// options.hooks.afterFile — только для самотестов: снимок «сбоя» после файла.
function setEnabled(installPath, id, enabled, options = {}) {
  const logger = options.logger || { info() {}, warn() {} };
  const pack = packOf(id);
  recover(installPath, options);

  const where = layout(installPath, pack);
  const state = readState(where.stateFile);
  const record = state.packs[pack.id] || (isAvailable(installPath, pack) ? adoptedRecord(pack, where) : null);
  if (!!record !== !!enabled) {
    if (enabled && !isAvailable(installPath, pack))
      throw new DeliveryError("CUSTOM_PACK_NOT_AVAILABLE", `Файлов пака ${pack.id} нет в игре`);
    if (enabled) enable(pack, state, where, options.hooks || {});
    else disable(pack, state, where, record, options.hooks || {});
    logger.info(`[custom-packs] пак ${pack.id} ${enabled ? "включён" : "выключен"}, файлов ${pack.files.length}`);
  }
  return list(installPath, pack.game);
}

// Звук для прослушивания: замена из пака или оригинал из папки игры.
function readSound(installPath, id, file, which) {
  const pack = packOf(id);
  const baseDir = GAMES[pack.game].baseDir;
  if (pack.kind !== "sounds" || !baseDir || pack.files.indexOf(file) === -1 || (which !== "pack" && which !== "original"))
    throw new DeliveryError("CUSTOM_PACK_FILE_UNKNOWN", `Нет звука ${String(file)} в паке ${pack.id}`);

  const full = which === "pack"
    ? layout(installPath, pack).source(file)
    : fileOps.resolveInside(installPath, `${baseDir}/${file}`);
  const stats = fs.statSync(full);
  if (!stats.isFile() || stats.size > MAX_SOUND_BYTES)
    throw new DeliveryError("CUSTOM_PACK_FILE_UNKNOWN", `Звук ${file} не прочитан`);
  return { dataUrl: `data:audio/wav;base64,${fs.readFileSync(full).toString("base64")}` };
}

module.exports = {
  GAMES,
  PACKS,
  STATE_DIR,
  hasPacks,
  targets,
  list,
  recover,
  setEnabled,
  readSound,
};
