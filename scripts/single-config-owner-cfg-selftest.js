"use strict";

// Регрессия «единого конфига» на настоящем autoexec.cfg владельца (сборка Ghosting из
// Speedrun Package 2013): UTF-8, CRLF, кириллица в комментариях, комментарии после значений,
// bind с «;», runtime-exec самого себя и exec отсутствующего gh_optimized.cfg в конце.
// Файл лежит в scripts/fixtures/single-config и копируется в тесты байт в байт.

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
const ghostingPersonalization = require("../src/main/game-delivery/ghosting-personalization");
const launchArgs = require("../src/main/game-delivery/launch-args");
const cfgDocument = require("../src/shared/cfg-document");
const cfgImport = require("../src/shared/cfg-import");
const gameBinds = require("../src/shared/game-binds");
const gameConfig = require("../src/shared/game-config");
const bindPresets = require("../src/shared/bind-presets");

const FIXTURE = path.join(__dirname, "fixtures", "single-config", "ghosting-package-autoexec.cfg");
const FIXTURE_SHA256 = "9b1e2e9ccbf5e3dd4c0a39c5ae371760dd62159e7818f09c2a5f4e752aee3acb";
const GAME = "ghosting-renewed";
const GAME_DIR = "ghostingmod";
const ENGINE = bindPresets.engineFor(GAME);
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };
const NOW = new Date(2026, 10, 6, 12, 34, 56);
const STAMP = "migration-20261106-123456";
const VERSION = "0.4.9";
const WIN_SAVE_BIND = 'bind h "save :; reload"';
const LINUX_SAVE_BIND = "save /; reload";

const original = fs.readFileSync(FIXTURE);
const text = original.toString("utf8");
const sha256Of = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");

function temp(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-owner-cfg-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return root;
}

const modDir = (root) => cfgSearch.modDirOf(root, GAME_DIR, ENGINE);
const playerFile = (root) => playerConfig.locate(root, GAME, GAME_DIR, EN).file;

function put(root, name, bytes) {
  const file = path.join(modDir(root), ...name.split("/"));
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes);
}

// Установка Ghosting с файлом владельца в ghostingmod/cfg/autoexec.cfg.
function install(t, bytes = original) {
  const root = temp(t);
  fs.mkdirSync(path.dirname(playerFile(root)), { recursive: true });
  fs.writeFileSync(playerFile(root), bytes);
  return root;
}

// Чтение cfg как в движке (копия reader из config-migration-selftest).
function reader(root) {
  const files = cfgSearch.filesOf(root, GAME_DIR, ENGINE, EN);
  return (name) => (files.exists(name) ? fs.readFileSync(files.pathOf(name), "utf8") : null);
}

const state = (root) => runChain(reader(root), gameConfig.ENGINE_ROOTS[ENGINE]);

function assertSame(before, after) {
  assert.deepEqual(after.cvars, before.cvars);
  assert.deepEqual(after.binds, before.binds);
  assert.deepEqual(after.aliases, before.aliases);
}

// Настройки 0.4.8: прицел включён и изменён, FOV 100, один бинд игрока поверх файла.
function settings048() {
  const settings = launchArgs.defaultSettings();
  settings.personalization = {
    ...settings.personalization, crossEnabled: true, crossSize: 14,
    extra: { ...settings.personalization.extra, fov_desired: 100 },
  };
  settings.binds = { j: { command: "+use", base: "" } };
  return settings;
}

// Установка в состоянии после запуска 0.4.8: файл владельца + exec лаунчера (CRLF, как ensureExecLine),
// hlsr-personalization.cfg и hlsr-binds.cfg из генераторов 0.4.8.
function install048(t, settings) {
  const root = install(t, Buffer.from(text + (text.endsWith("\n") ? "" : "\r\n") + "exec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n", "utf8"));
  const clean = gameBinds.sanitizeOverrides(settings.binds);
  const { aliases } = gameBindsInstall.scriptAliases(root, GAME, GAME_DIR, ENGINE, clean, fs);
  put(root, "hlsr-binds.cfg", gameBinds.toCfgLines(settings.binds, { engine: ENGINE, aliases }));
  put(root, "hlsr-personalization.cfg",
    ghostingPersonalization.toCfgLines(ghostingPersonalization.sanitizePersonalization(settings.personalization)));
  return root;
}

const migrate = (root, settings, extra = {}) =>
  configMigration.migrate(root, GAME, GAME_DIR, { ...EN, settings, launcherVersion: VERSION, now: NOW, ...extra });

const fovLines = (lines) => lines.map((line, index) => [index, line]).filter(([, line]) => /^(default_fov|fov_desired|fov) /.test(line));

test("файл владельца: копия фикстуры совпадает с эталоном по sha256", () => {
  assert.equal(original.length, 3985);
  assert.equal(sha256Of(original), FIXTURE_SHA256);
});

test("круговой разбор: serialize(parse) байт в байт; set(default_fov) меняет ровно одну строку", () => {
  assert.equal(cfgDocument.serialize(cfgDocument.parse(text)), text);
  assert.deepEqual(Buffer.from(cfgDocument.serialize(cfgDocument.parse(text)), "utf8"), original);

  const doc = cfgDocument.parse(text);
  assert.equal(cfgDocument.get(doc, "cvar:default_fov").value, "110");
  const changed = cfgDocument.serialize(cfgDocument.set(doc, "cvar:default_fov", "100", "personalization"));
  const before = text.split("\r\n");
  const after = changed.split("\r\n");
  assert.equal(after.length, before.length);
  const diff = before.map((line, index) => index).filter((index) => before[index] !== after[index]);
  assert.equal(diff.length, 1);
  assert.equal(before[diff[0]], "default_fov 110");
  assert.equal(after[diff[0]], "default_fov 100");
  // Остальные байты те же: вернув значение, получаем исходные байты.
  assert.deepEqual(Buffer.from(changed.replace("default_fov 100", "default_fov 110"), "utf8"), original);
});

test("isStock: файл владельца не стоковый; ensure оставляет его байт в байт", (t) => {
  assert.equal(playerConfig.isStock(GAME, text), false);
  const root = install(t);
  const result = playerConfig.ensure(root, GAME, GAME_DIR, EN);
  assert.equal(result.created, false);
  assert.equal(result.file, playerFile(root));
  assert.deepEqual(fs.readFileSync(playerFile(root)), original);
});

test("переход с 0.4.8: итог в игре тот же, кириллица на месте, exec лаунчера убраны, файлы в копии", (t) => {
  const settings = settings048();
  const root = install048(t, settings);
  const before = state(root);
  // Проверяем, что исходное состояние несёт настройки 0.4.8 поверх файла владельца.
  assert.equal(before.cvars.get("default_fov"), "100");
  assert.equal(before.cvars.get("gh_cross"), "1");
  assert.equal(before.cvars.get("gh_cross_size"), "14");
  assert.equal(before.binds.get("j"), "+use");
  assert.equal(before.binds.get("u"), "exec autoexec");
  assert.equal(before.binds.get("t"), "bind mwheeldown +duck; bind mwheelup +duck");
  assert.equal(before.cvars.get("gh_name"), "myrun");

  const previous = fs.readFileSync(playerFile(root));
  assert.equal(configMigration.needsMigration(root, GAME, GAME_DIR, EN), true);
  const result = migrate(root, settings, { platform: "win32" });
  assert.equal(result.status, "migrated");
  assertSame(before, state(root));

  const migrated = fs.readFileSync(playerFile(root));
  for (const line of text.split("\r\n").filter((item) => /[^\x00-\x7f]/.test(item)))
    assert.ok(migrated.includes(Buffer.from(line, "utf8")), `строка потеряна: ${line}`);
  assert.deepEqual(migrated.subarray(0, original.length), original, "файл владельца в начале байт в байт (C3)");
  const migratedText = migrated.toString("utf8");
  assert.doesNotMatch(migratedText, /^\s*exec\s+hlsr-/im);
  assert.match(migratedText, /^bind h "save :; reload"/m, "на Windows бинд удаления сейва не меняется");
  assert.match(migratedText, /^exec gh_optimized\.cfg\r?$/m, "exec отсутствующего файла остаётся");

  // hlsr-*.cfg ушли в копию, прежний файл игрока в ней байт в байт.
  assert.deepEqual(fs.readdirSync(modDir(root)).filter((name) => /^hlsr-.*\.cfg$/i.test(name)), []);
  const backup = path.join(root, ".hlsr", "backup", STAMP, GAME_DIR, "cfg");
  assert.equal(result.backupDir, path.join(root, ".hlsr", "backup", STAMP));
  assert.ok(fs.existsSync(path.join(backup, "hlsr-personalization.cfg")));
  assert.ok(fs.existsSync(path.join(backup, "hlsr-binds.cfg")));
  assert.deepEqual(fs.readFileSync(path.join(backup, "autoexec.cfg")), previous);
  assert.equal(configMigration.needsMigration(root, GAME, GAME_DIR, EN), false);
});

test("переход с 0.4.8 на Linux: бинд удаления сейва — вариант для Linux, остальное как на Windows", (t) => {
  const settings = settings048();
  const root = install048(t, settings);
  const before = state(root);
  assert.equal(migrate(root, settings, { platform: "linux" }).status, "migrated");
  const after = state(root);
  assert.equal(after.binds.get("h"), LINUX_SAVE_BIND);
  assert.equal(before.binds.get("h"), "save :; reload");
  // Всё, кроме одного бинда, как в 0.4.8.
  const expected = new Map(before.binds).set("h", LINUX_SAVE_BIND);
  assert.deepEqual(after.binds, expected);
  assert.deepEqual(after.cvars, before.cvars);
  assert.deepEqual(after.aliases, before.aliases);
  assert.ok(!fs.readFileSync(playerFile(root), "utf8").includes(WIN_SAVE_BIND));
});

test("персонализация: FOV и gh_* читаются из файла; смена FOV пишет ровно default_fov, fov_desired и fov", (t) => {
  const root = install(t);
  const stored = launchArgs.defaultSettings();
  const shown = playerConfig.readPersonalization(root, GAME, GAME_DIR, stored, EN);
  const extra = shown.personalization.extra;
  assert.equal(extra.fov_desired, 110);
  assert.equal(extra.viewmodel_fov, 62);
  assert.equal(extra.gh_name, "myrun");
  assert.equal(extra.gh_opacity, 230);
  assert.equal(extra.gh_runcomplete_notify, false);
  assert.equal(extra.gh_timer, true);
  assert.equal(shown.personalization.hideQuickInfo, false, "hud_quickinfo 1 в файле");

  const next = { ...shown, personalization: { ...shown.personalization, extra: { ...extra, fov_desired: 100 } } };
  const result = playerConfig.writePersonalization(root, GAME, GAME_DIR, stored, next, EN);
  // Генератор Ghosting связывает FOV тремя строками (ghosting-settings.js: default_fov, fov_desired, fov).
  assert.deepEqual(result.changed, ["default_fov", "fov_desired", "fov"]);
  const after = fs.readFileSync(playerFile(root), "utf8");
  const a = text.split("\r\n");
  const b = after.split("\r\n");
  assert.equal(b.length, a.length);
  const diff = a.map((line, index) => index).filter((index) => a[index] !== b[index]);
  assert.deepEqual(diff.map((index) => [a[index], b[index]]),
    [["default_fov 110", "default_fov 100"], ["fov_desired 110", "fov_desired 100"], ["fov 110", "fov 100"]]);
  assert.deepEqual(fovLines(b).map(([index]) => index), diff);

  // Повторная запись того же — файл не трогается.
  const again = playerConfig.writePersonalization(root, GAME, GAME_DIR, next, next, EN);
  assert.deepEqual(again.changed, []);
  assert.equal(fs.readFileSync(playerFile(root), "utf8"), after);
});

test("персонализация после перехода: значения 0.4.8 (FOV 100, прицел) читаются из слитого файла", (t) => {
  const settings = settings048();
  const root = install048(t, settings);
  migrate(root, settings, { platform: "win32" });
  const shown = playerConfig.readPersonalization(root, GAME, GAME_DIR, launchArgs.defaultSettings(), EN);
  assert.equal(shown.personalization.extra.fov_desired, 100);
  assert.equal(shown.personalization.crossEnabled, true);
  assert.equal(shown.personalization.crossSize, 14);
  assert.equal(shown.personalization.extra.gh_name, "myrun");
});

// Файл так, как его отдаёт окно выбора (index.js pickBindsConfig): байты как latin1, без BOM.
const picked = gameBinds.cfgText(original.toString("latin1"));

test("импорт: gh_optimized.cfg отсутствует — закомментирован; runtime-exec не раскрыт; итог тот же; undo возвращает байты", (t) => {
  const flat = (missing) => cfgImport.flattenImport({ engine: ENGINE, root: { name: "autoexec.cfg", text: picked }, files: {}, missing });
  const built = flat(["gh_optimized.cfg"]);
  assert.match(built.text, /^\/\/ HLSR: exec gh_optimized\.cfg skipped \(not imported\)$/m);
  assert.doesNotMatch(built.text, /^exec gh_optimized/m);
  assert.match(built.text, /^bind u "exec autoexec"/m, "runtime-bind остаётся биндом");
  assert.doesNotMatch(built.text, /===== from /);
  assert.deepEqual(built.runtimeFiles, []);
  assert.equal(built.report.cycles.length, 0);

  // Не заявлен отсутствующим, но и не приложен — строка остаётся как есть.
  assert.match(flat([]).text, /^exec gh_optimized\.cfg$/m);

  const chain = (body) => runChain((name) => (name === "autoexec.cfg" ? body : null), ["autoexec.cfg"]);
  assertSame(chain(picked), chain(built.text));
  assert.equal(chain(built.text).binds.get("u"), "exec autoexec");

  const root = install(t);
  const options = { ...EN, now: new Date(2026, 10, 6, 12, 0, 0).getTime(), backupsFile: path.join(temp(t), "backups.json") };
  const sha = playerConfig.read(root, GAME, GAME_DIR, options).sha256;
  const result = gameBindsInstall.importPlayerConfig(root, GAME, GAME_DIR, built, { ...options, expectedSha256: sha });
  assert.equal(result.status, "ok");
  assert.deepEqual(fs.readFileSync(playerFile(root)), Buffer.from(built.text.replace(/\r?\n/g, "\r\n"), "latin1"));
  assert.doesNotMatch(fs.readFileSync(playerFile(root), "latin1"), /Your own/, "импорт Ghosting без заголовка «Your own» (R16)");
  assert.match(result.backup.replace(/\\/g, "/"), /^\.hlsr\/backup\/20261106-120000$/);
  assert.deepEqual(fs.readFileSync(path.join(root, result.backup, GAME_DIR, "cfg", "autoexec.cfg")), original);

  assert.equal(gameBindsInstall.undoImport(root, GAME, GAME_DIR, options).status, "ok");
  assert.deepEqual(fs.readFileSync(playerFile(root)), original);
});

test("C3: импорт файла владельца через окно выбора — байт в байт, кириллица UTF-8 не кодируется второй раз (проба 6)", (t) => {
  const built = cfgImport.flattenImport({ engine: ENGINE, root: { name: "autoexec.cfg", text: picked }, files: {}, missing: [] });
  const root = install(t, Buffer.from("// старый autoexec\r\n", "utf8"));
  const result = gameBindsInstall.importPlayerConfig(root, GAME, GAME_DIR, built, EN);
  assert.equal(result.status, "ok");
  assert.deepEqual(fs.readFileSync(playerFile(root)), original);
  // Окно показывает значения из файла как UTF-8.
  const shown = playerConfig.readPersonalization(root, GAME, GAME_DIR, launchArgs.defaultSettings(), EN);
  assert.equal(shown.personalization.extra.fov_desired, 110);
});