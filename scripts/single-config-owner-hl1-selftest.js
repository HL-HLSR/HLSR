"use strict";

// Регрессия «единого конфига» на настоящем конфиге HL1 владельца: 1.cfg (латиница + «°»,
// LF) зовёт p2pv2.cfg и tc_n.cfg из bind/alias, сам себя (bind - "exec 1.cfg", alias start_run)
// и autoexec.cfg. Источники лежат в scripts/fixtures/single-config/hl1 (копии байт в байт).

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const { runChain } = require("./lib/cfg-state");
const configMigration = require("../src/main/game-delivery/config-migration");
const playerConfig = require("../src/main/game-delivery/player-config");
const cfgSearch = require("../src/main/game-delivery/cfg-search");
const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const goldsrcPersonalization = require("../src/main/game-delivery/goldsrc-personalization");
const hl1Default = require("../src/main/game-delivery/hl1-default-userconfig");
const launchArgs = require("../src/main/game-delivery/launch-args");
const cfgDocument = require("../src/shared/cfg-document");
const cfgImport = require("../src/shared/cfg-import");
const gameBinds = require("../src/shared/game-binds");

const DIR = path.join(__dirname, "fixtures", "single-config", "hl1");
const SHA256 = {
  "1.cfg": "3913c20845898d82f03fdd9609f9ce7382d5d82813bfe7e183a47b6ecc443d6c",
  "p2pv2.cfg": "d71e36a69b51fcda19b0f9397f073193a27fbb7eb137e8e82b6c607a4d6d2f51",
  "tc_n.cfg": "3e889b86f9a9153c3a9d871681d21b801d44355220dd061b16897c5334610b02",
  "autoexec.cfg": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "userconfig.cfg": "cc3bbef9d825def9ae6a93b61cde2ed27d7a8400cad602d8b57b318f4127f019",
  "WON.cfg": "c8ae42847be66f2206756fc67ce65309fcb32c356066cdf948ca825fe148e72e",
};
const SIZE = { "1.cfg": 6923, "p2pv2.cfg": 2560, "tc_n.cfg": 1349, "autoexec.cfg": 0, "userconfig.cfg": 68, "WON.cfg": 218 };
const GAME = "hl1-won";
const GAME_DIR = "Half-Life/valve_WON";
const ENGINE = "goldsrc";
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };
const NOW = new Date(2026, 10, 6, 12, 34, 56);
const STAMP = "migration-20261106-123456";
const IMPORT_NOW = new Date(2026, 10, 6, 12, 0, 0).getTime();
const RUNTIME_FILES = ["1.cfg", "p2pv2.cfg", "tc_n.cfg"];
// Какую копию импорта записал лаунчер — в папке лаунчера, не в папке игры.
const LAUNCHER_DIR = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-owner-hl1-launcher-"));
test.after(() => fs.rmSync(LAUNCHER_DIR, { recursive: true, force: true }));

const bytesOf = (name) => fs.readFileSync(path.join(DIR, name));
const textOf = (name) => bytesOf(name).toString("latin1");
const sha256Of = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");

function temp(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-owner-hl1-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
}

const modDir = (root) => cfgSearch.modDirOf(root, GAME_DIR, ENGINE);
const playerFile = (root) => playerConfig.locate(root, GAME, GAME_DIR, EN).file;
const put = (root, name, bytes) => {
  fs.mkdirSync(modDir(root), { recursive: true });
  fs.writeFileSync(path.join(modDir(root), name), bytes);
};

// Папка игры владельца до импорта: его userconfig.cfg (exec WON.cfg) и WON.cfg.
function installOwner(t) {
  const root = temp(t);
  put(root, "userconfig.cfg", bytesOf("userconfig.cfg"));
  put(root, "WON.cfg", bytesOf("WON.cfg"));
  return root;
}

// Чтение cfg как в движке из папки мода.
function reader(root) {
  const files = cfgSearch.filesOf(root, GAME_DIR, ENGINE, EN);
  return (name) => (files.exists(name) ? fs.readFileSync(files.pathOf(name), "latin1") : null);
}

// Цепочка, как её строит окно: файлы по очереди, каждый берётся из фикстур.
function walk() {
  const root = { name: "1.cfg", text: textOf("1.cfg") };
  const files = {};
  const asked = [];
  for (;;) {
    const wanted = cfgImport.nextWanted({ engine: ENGINE, root, files, missing: [], topLevelOnly: false });
    if (!wanted) break;
    asked.push(wanted.key);
    files[wanted.key] = { name: wanted.name, target: wanted.name, text: textOf(wanted.name) };
  }
  return { root, files, asked };
}

const flatten = () => {
  const { root, files } = walk();
  return cfgImport.flattenImport({ engine: ENGINE, root, files });
};

function assertSame(before, after) {
  assert.deepEqual(after.cvars, before.cvars);
  assert.deepEqual(after.binds, before.binds);
  assert.deepEqual(after.aliases, before.aliases);
}

// Состояние «игрок выполнил 1.cfg у себя»: все файлы фикстур под своими именами.
const sourceChain = () => runChain((name) => (name in SHA256 ? textOf(name) : null), ["1.cfg"]);
// Разделы шаблона, которые импорт HL1 ставит перед текстом игрока (R12, R16).
const HEAD = playerConfig.templateLines(GAME, ["rules", "settings"]).join("\r\n") + "\r\n\r\n";
// То же, но 1.cfg выполняется после правил и настроек шаблона (runChain даёт имя в нижнем регистре).
const headChain = () => runChain((name) => {
  const file = Object.keys(SHA256).find((item) => item.toLowerCase() === name);
  return name === "1.cfg" ? HEAD + textOf(name) : file ? textOf(file) : null;
}, ["1.cfg"]);

function importInto(root) {
  const options = { ...EN, now: IMPORT_NOW, backupsFile: path.join(LAUNCHER_DIR, "player-config-backups.json") };
  const sha = playerConfig.read(root, GAME, GAME_DIR, options).sha256;
  return { options, result: gameBindsInstall.importPlayerConfig(root, GAME, GAME_DIR, flatten(), { ...options, expectedSha256: sha }) };
}

test("фикстуры: размеры и sha256 совпадают с исходными файлами владельца", () => {
  for (const name of Object.keys(SHA256)) {
    assert.equal(bytesOf(name).length, SIZE[name], name);
    assert.equal(sha256Of(bytesOf(name)), SHA256[name], name);
  }
  assert.ok(bytesOf("1.cfg").includes(Buffer.from([0xc2, 0xb0])), "в 1.cfg есть «°» (UTF-8, два байта)");
});

test("1.cfg как latin1: serialize(parse) байт в байт; set(default_fov) меняет ровно одну строку", () => {
  const text = textOf("1.cfg");
  assert.equal(cfgDocument.serialize(cfgDocument.parse(text)), text);
  const doc = cfgDocument.parse(text);
  assert.equal(cfgDocument.get(doc, "cvar:default_fov").value, "110");
  const changed = cfgDocument.serialize(cfgDocument.set(doc, "cvar:default_fov", "100", "personalization"));
  const before = text.split("\n");
  const after = changed.split("\n");
  assert.equal(after.length, before.length);
  const diff = before.map((line, index) => index).filter((index) => before[index] !== after[index]);
  assert.deepEqual(diff.map((index) => [before[index], after[index]]), [["default_fov 110", "default_fov 100"]]);
  assert.deepEqual(Buffer.from(changed, "latin1"), Buffer.from(text.replace("default_fov 110", "default_fov 100"), "latin1"));
  assert.deepEqual(Buffer.from(changed.replace("default_fov 100", "default_fov 110"), "latin1"), bytesOf("1.cfg"), "«°» не тронут");
});

test("цепочка импорта: окно просит p2pv2.cfg, autoexec.cfg и tc_n.cfg; 1.cfg (корень) и userconfig.cfg не просит", () => {
  const { asked } = walk();
  // Порядок — как файлы встречаются в 1.cfg. autoexec.cfg окно спрашивает (не системный), корень 1.cfg пропускается.
  assert.deepEqual(asked, ["p2pv2.cfg", "autoexec.cfg", "tc_n.cfg"]);
});

test("импорт одним файлом: userconfig.cfg выполняет то же, что 1.cfg; вызываемые из bind/alias файлы лежат в папке мода", (t) => {
  const root = installOwner(t);
  const built = flatten();
  assert.deepEqual(built.runtimeFiles.map((item) => item.file).sort(), [...RUNTIME_FILES].sort());
  const { result } = importInto(root);
  assert.equal(result.status, "ok");

  // Корень импорта — сам userconfig.cfg; прежнее «exec WON.cfg» владельца заменено, WON.cfg остался на диске.
  const player = fs.readFileSync(playerFile(root), "latin1");
  assert.equal(playerFile(root), path.join(modDir(root), "userconfig.cfg"));
  // Прежний exec WON.cfg игрока заменён строкой раздела правил (R12), WON.cfg остался на диске.
  assert.equal(player.match(/^\s*exec\s+WON\.cfg/gim).length, 1);
  assert.ok(fs.existsSync(path.join(modDir(root), "WON.cfg")));
  assert.match(player, /bind - "exec 1\.cfg;say RESET CFG"/);
  assert.match(player, /exec autoexec\.cfg; exec 1\.cfg/, "start_run остаётся alias со своими exec");
  assert.ok(Buffer.from(player, "latin1").includes(Buffer.from([0xc2, 0xb0])), "«°» на месте");

  // Цепочка = правила WON.cfg и настройки шаблона (R12, R16), затем 1.cfg: значения 1.cfg не потеряны.
  const imported = runChain(reader(root), ["userconfig.cfg"]);
  const source = sourceChain();
  for (const [name, value] of source.cvars) if (!["fps_max", "clockwindow", "bxt_hud_incorrect_fps_indicator"].includes(name)) assert.equal(imported.cvars.get(name), value, name);
  assertSame(headChain(), imported);

  // Всё, что игра вызывает по клавише, существует: 1.cfg (bind -, bind p, start_run), p2pv2.cfg, tc_n.cfg.
  const called = cfgImport.execsOf(player, ENGINE).filter((item) => item.runtime && !cfgImport.ENGINE_RUN.test(item.key));
  assert.deepEqual([...new Set(called.map((item) => item.key))].sort(), [...RUNTIME_FILES].sort());
  for (const name of RUNTIME_FILES) {
    const file = path.join(modDir(root), name);
    assert.ok(fs.existsSync(file), `${name} не скопирован в папку мода`);
    // 1.cfg после слияния отличается только переводами строк (CRLF в папке мода).
    assert.equal(fs.readFileSync(file, "latin1").replace(/\r\n/g, "\n"), textOf(name).replace(/\r\n/g, "\n"), name);
  }
});

test("autoexec.cfg игрока импорт в папку мода не копирует (его выполняет сам движок при старте)", (t) => {
  const root = installOwner(t);
  importInto(root);
  // alias start_run зовёт exec autoexec.cfg: файл остаётся тем, что есть у игрока (здесь — его нет).
  assert.equal(fs.existsSync(path.join(modDir(root), "autoexec.cfg")), false);
  const built = flatten();
  assert.ok(built.report.files.includes("autoexec.cfg"), "в отчёте импорта файл числится");
  assert.ok(!built.runtimeFiles.some((item) => item.file === "autoexec.cfg"));
  // Если autoexec.cfg уже лежит в папке мода (как у владельца, 0 байт), импорт его не трогает.
  const own = installOwner(t);
  put(own, "autoexec.cfg", bytesOf("autoexec.cfg"));
  importInto(own);
  assert.equal(fs.readFileSync(path.join(modDir(own), "autoexec.cfg")).length, 0);
});

test("undo: возвращает userconfig.cfg владельца байт в байт и удаляет скопированные файлы", (t) => {
  const root = installOwner(t);
  const { options } = importInto(root);
  for (const name of RUNTIME_FILES) assert.ok(fs.existsSync(path.join(modDir(root), name)), name);
  const undone = gameBindsInstall.undoImport(root, GAME, GAME_DIR, options);
  assert.equal(undone.status, "ok");
  assert.deepEqual(fs.readFileSync(playerFile(root)), bytesOf("userconfig.cfg"));
  assert.deepEqual(fs.readFileSync(path.join(modDir(root), "WON.cfg")), bytesOf("WON.cfg"));
  for (const name of RUNTIME_FILES) assert.equal(fs.existsSync(path.join(modDir(root), name)), false, `${name} остался`);
});

test("undo (R18): текущие файлы сначала уходят в новую копию — правки после импорта не теряются, возврат можно вернуть", (t) => {
  const root = installOwner(t);
  const { options } = importInto(root);
  // После импорта игрок сохранил бинд в «Клавиатуре».
  gameBindsInstall.applyBinds(root, GAME, GAME_DIR, { k: { command: "+duck", base: "" } }, { ...EN, previous: {} });
  const edited = fs.readFileSync(playerFile(root));
  const runtime = Object.fromEntries(RUNTIME_FILES.map((name) => [name, fs.readFileSync(path.join(modDir(root), name))]));

  assert.equal(gameBindsInstall.undoImport(root, GAME, GAME_DIR, options).status, "ok");
  assert.deepEqual(fs.readFileSync(playerFile(root)), bytesOf("userconfig.cfg"));
  // «Вернуть прежний» ещё раз — файлы после импорта вместе с правкой.
  assert.equal(gameBindsInstall.undoImport(root, GAME, GAME_DIR, options).status, "ok");
  assert.deepEqual(fs.readFileSync(playerFile(root)), edited);
  for (const name of RUNTIME_FILES) assert.deepEqual(fs.readFileSync(path.join(modDir(root), name)), runtime[name], name);
});

test("персонализация после импорта: FOV, прицел BXT, цвет, зазор и размер из файла показываются", (t) => {
  const root = installOwner(t);
  importInto(root);
  const stored = launchArgs.defaultSettings();
  const shown = playerConfig.readPersonalization(root, GAME, GAME_DIR, stored, EN);
  assert.equal(stored.bxt.fov, 90, "хранилище не меняется");
  assert.equal(shown.bxt.fov, 110);
  assert.equal(shown.bxt.viewangles, true, "bxt_hud_viewangles 1");
  const look = shown.personalization;
  assert.equal(look.crossEnabled, true, "bxt_cross 1");
  assert.deepEqual(look.crossColor, { r: 255, g: 255, b: 255 });
  assert.equal(look.crossSize, 7);
  // Отрицательный зазор BXT принимается (диапазон -30..30), он показывается как в файле.
  assert.equal(look.crossGap, -4);
  assert.equal(look.crossPreset, "custom");
  // crosshair 0 вместе с bxt_cross 1: родной прицел скрывает сам BXT-прицел (crossHideDefault), отдельный флаг не нужен.
  assert.equal(shown.bxt.hideCrosshair, false);
  assert.equal(look.crossHideDefault, true);
});

// Смещение модели оружия (bxt_viewmodel_ofs_*) берётся из файла, как остальные значения.
test("персонализация после импорта: смещение viewmodel (bxt_viewmodel_ofs_up -2, forward 2) показывается как в файле", (t) => {
    const root = installOwner(t);
    importInto(root);
    const shown = playerConfig.readPersonalization(root, GAME, GAME_DIR, launchArgs.defaultSettings(), EN);
    assert.equal(shown.bxt.viewmodelUp, -2);
    assert.equal(shown.bxt.viewmodelForward, 2);
  });

test("импорт HL1 сохраняет правила категории: exec WON.cfg первой командой, fps_max 100 и clockwindow 0 выполняются", (t) => {
  const root = installOwner(t);
  importInto(root);
  const player = fs.readFileSync(playerFile(root), "latin1");
  // Первая команда — строка раздела правил шаблона (exec WON.cfg с комментарием, R16).
  assert.match(player.split(/\r?\n/).find((line) => line.trim() && !line.startsWith("//")).trim().toLowerCase(), /^exec won\.cfg\b/);
  assert.match(player, /^\/\/ ===== HLSR: category rules =====\r?\n/);
  const state = runChain(reader(root), ["userconfig.cfg"]);
  assert.equal(state.cvars.get("fps_max"), "100");
  assert.equal(state.cvars.get("clockwindow"), "0");
});

test("импорт 1.cfg (R16): WON.cfg, затем настройки шаблона, затем текст владельца; бинд, сохранённый после, — последний бинд верхнего уровня", (t) => {
  const root = installOwner(t);
  importInto(root);
  const player = fs.readFileSync(playerFile(root), "latin1");
  const at = (needle) => player.indexOf(needle);
  assert.ok(at("exec WON.cfg") >= 0 && at("exec WON.cfg") < at(cfgDocument.SECTIONS.settings), "правила первыми");
  assert.ok(at(cfgDocument.SECTIONS.settings) < at("cl_showfps 1") && at("cl_showfps 1") < at("name prolert"), "настройки шаблона до текста владельца");
  assert.ok(player.startsWith(HEAD), "разделы шаблона как в новом файле");
  assert.doesNotMatch(player, /Your own/, "без заголовка «Your own»");
  const state = runChain(reader(root), ["userconfig.cfg"]);
  // Шаблон даёт то, чего нет в 1.cfg; бинд p и всё остальное — из 1.cfg.
  assert.equal(state.cvars.get("cl_showfps"), "1");
  assert.equal(state.cvars.get("_bxt_save_runtime_data_in_demos"), "1");
  assert.ok(state.aliases.has("run"));
  assert.equal(state.binds.get("p"), sourceChain().binds.get("p"));

  // Бинд из «Клавиатуры» после импорта: в конце файла, после всего текста владельца.
  const sha = playerConfig.read(root, GAME, GAME_DIR, EN).sha256;
  gameBindsInstall.applyBinds(root, GAME, GAME_DIR, { k: { command: "+duck", base: "" } }, { ...EN, previous: {}, expectedSha256: sha });
  const after = fs.readFileSync(playerFile(root), "latin1");
  assert.ok(after.startsWith(player.replace(/\s+$/, "")), "текст импорта не тронут");
  const binds = after.split(/\r?\n/).filter((line) => /^\s*bind\s/i.test(line));
  assert.equal(binds[binds.length - 1], 'bind "k" "+duck"');
  assert.equal(runChain(reader(root), ["userconfig.cfg"]).binds.get("k"), "+duck");
});

test("импорт HL1: корень уже выполняет WON.cfg на верхнем уровне - второй строки нет", (t) => {
  const root = installOwner(t);
  const sha = playerConfig.read(root, GAME, GAME_DIR, EN).sha256;
  const built = { text: "Exec won.cfg\r\nfps_max 72\r\n", runtimeFiles: [] };
  const result = gameBindsInstall.importPlayerConfig(root, GAME, GAME_DIR, built, { ...EN, now: IMPORT_NOW, expectedSha256: sha });
  assert.equal(result.status, "ok");
  const player = fs.readFileSync(playerFile(root), "latin1");
  assert.equal(player.match(/^\s*exec\s+won\.cfg/gim).length, 1);
  assert.doesNotMatch(player, /category rules/);
  assert.ok(player.startsWith(cfgDocument.SECTIONS.settings), "настройки шаблона остаются (I5)");
});

test("переход с 0.4.8: цепочка 1.cfg + hlsr-*.cfg даёт тот же итог, вызываемые файлы на месте", (t) => {
  const root = temp(t);
  for (const name of ["1.cfg", "p2pv2.cfg", "tc_n.cfg", "autoexec.cfg", "WON.cfg"]) put(root, name, bytesOf(name));
  put(root, "config.cfg", 'fps_max "72"\r\nexec userconfig.cfg\r\n');
  // Запись импорта 0.4.8: exec 1.cfg выполняется из userconfig.cfg, остальные файлы — вызываются в игре.
  fs.mkdirSync(path.join(root, ".hlsr"), { recursive: true });
  fs.writeFileSync(path.join(root, ".hlsr", "cfg-import.json"), JSON.stringify({
    execs: ["1.cfg"], files: ["1.cfg", "p2pv2.cfg", "tc_n.cfg", "autoexec.cfg"].map((name) => ({ path: name })),
  }, null, 2));
  put(root, "userconfig.cfg", hl1Default.TEXT + 'exec 1.cfg\r\nexec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n');

  const settings = launchArgs.defaultSettings();
  settings.bxt = Object.assign({}, settings.bxt, { fov: 100, timer: true });
  settings.binds = { g: { command: "+use", base: "" } };
  const clean = gameBinds.sanitizeOverrides(settings.binds);
  const { aliases } = gameBindsInstall.scriptAliases(root, GAME, GAME_DIR, ENGINE, clean, fs);
  put(root, "hlsr-binds.cfg", Buffer.from(gameBinds.toCfgLines(settings.binds, { engine: ENGINE, aliases }), "latin1"));
  put(root, "hlsr-personalization.cfg", Buffer.from(goldsrcPersonalization.toCfgLines(settings), "latin1"));

  const state = () => runChain(reader(root), ["config.cfg"]);
  const before = state();
  assert.equal(before.binds.get("-"), "exec 1.cfg;say RESET CFG");
  assert.equal(before.binds.get("g"), "+use");
  assert.equal(before.cvars.get("default_fov"), "100", "hlsr-personalization.cfg 0.4.8 перекрывает 110 из 1.cfg");

  assert.equal(configMigration.needsMigration(root, GAME, GAME_DIR, EN), true);
  const result = configMigration.migrate(root, GAME, GAME_DIR, { ...EN, settings, launcherVersion: "0.4.9", now: NOW });
  assert.equal(result.status, "migrated");
  assertSame(before, state());

  const migrated = fs.readFileSync(playerFile(root), "latin1");
  assert.doesNotMatch(migrated, /^\s*exec\s+hlsr-/im);
  assert.doesNotMatch(migrated, /^\s*exec\s+1\.cfg/im, "exec 1.cfg раскрыт на месте");
  assert.ok(Buffer.from(migrated, "latin1").includes(Buffer.from([0xc2, 0xb0])), "«°» на месте");
  assert.deepEqual(fs.readdirSync(modDir(root)).filter((name) => /^hlsr-.*\.cfg$/i.test(name)), []);
  assert.ok(fs.existsSync(path.join(root, ".hlsr", "backup", STAMP, "Half-Life", "valve_WON", "hlsr-binds.cfg")));
  // Файлы, которые зовут bind и alias (в том числе сам 1.cfg: bind - и start_run), остались.
  for (const name of ["1.cfg", "p2pv2.cfg", "tc_n.cfg", "autoexec.cfg"]) {
    assert.deepEqual(fs.readFileSync(path.join(modDir(root), name)), bytesOf(name), name);
  }
});