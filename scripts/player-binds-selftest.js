"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const playerConfig = require("../src/main/game-delivery/player-config");
const cfgDocument = require("../src/shared/cfg-document");
const bindPresets = require("../src/shared/bind-presets");
const hl1Guide = require("../src/shared/hl1-guide");

const HL1 = "Half-Life/valve_WON";
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };

function setup(t, text) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-player-binds-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const dir = path.join(root, "Half-Life", "valve_WON");
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, "userconfig.cfg");
  if (text !== undefined) fs.writeFileSync(file, text, "latin1");
  return { root, dir, file };
}

const apply = (env, binds, options = {}) =>
  gameBindsInstall.applyBinds(env.root, "hl1-won", HL1, binds, { ...EN, ...options });
// Кириллица в тестах — байты UTF-8, прочитанные как latin1, как в файле игрока.
const raw = (value) => Buffer.from(value, "utf8").toString("latin1");
const text = (env) => fs.readFileSync(env.file, "latin1");

test("бинд: строка появляется в разделе binds, остальные байты на месте", (t) => {
  // 0x80–0xFF в комментарии (cp1251) переживают правку.
  const before = Buffer.from(
    "// ===== HLSR: binds =====\r\nbind \"w\" \"+forward\"\r\n\r\n// ===== Your own =====\r\n// \xcf\xf0\xe8\xe2\xe5\xf2\r\n",
    "latin1"
  ).toString("latin1");
  const env = setup(t, before);
  const result = apply(env, { f: { command: "+duck", base: "" } });
  assert.equal(result.status, "applied");
  assert.equal(result.file, env.file);
  assert.deepEqual(result.keptEdited, []);
  const after = text(env);
  assert.equal(
    after,
    before.replace('bind "w" "+forward"\r\n', 'bind "w" "+forward"\r\nbind "f" "+duck"\r\n')
  );
  assert.equal(fs.existsSync(path.join(env.dir, "hlsr-binds.cfg")), false, "hlsr-binds.cfg не создаётся");
});

test("бинд: повторное сохранение того же ничего не меняет", (t) => {
  const env = setup(t, "// ===== HLSR: binds =====\r\n\r\n// ===== Your own =====\r\n");
  const binds = { f: { command: "+duck", base: "" }, g: { command: "", base: "+use" } };
  apply(env, binds);
  const once = text(env);
  apply(env, binds, { previous: binds });
  assert.equal(text(env), once);
  assert.match(once, /unbind "g"/, "пустая команда — unbind");
});

test("бинд: снятое назначение удаляет свою строку и не трогает чужие", (t) => {
  const env = setup(t, "// ===== HLSR: binds =====\r\nbind \"w\" \"+forward\"\r\n");
  const first = { f: { command: "+duck", base: "" } };
  apply(env, first);
  assert.match(text(env), /bind "f" "\+duck"/);
  apply(env, {}, { previous: first });
  assert.equal(text(env), "// ===== HLSR: binds =====\r\nbind \"w\" \"+forward\"\r\n");
});

test("бинд: уже назначенную игроком клавишу правим на месте", (t) => {
  const env = setup(t, raw('bind "f" "+use" // моя\r\nfps_max 100\r\n'));
  apply(env, { f: { command: "+duck", base: "+use" } });
  assert.equal(text(env), raw('bind "f" "+duck" // моя\r\nfps_max 100\r\n'));
});

test("скрипт гайда: блок HLSR script <alias>, снятие удаляет блок", (t) => {
  const env = setup(t, "// ===== HLSR: scripts =====\r\n\r\n// ===== Your own =====\r\n");
  const first = { f: { command: "+fps_20_hold", base: "" } };
  apply(env, first);
  const doc = cfgDocument.parse(text(env));
  const block = cfgDocument.findBlock(doc, "+fps_20_hold");
  assert.ok(block, "блок есть");
  assert.deepEqual(block.body, ['alias +fps_20_hold "fps_max 20; alias -fps_20_hold fps_20_reset"']);
  assert.ok(text(env).indexOf("HLSR script +fps_20_hold begin") < text(env).indexOf("Your own"), "блок в разделе scripts");
  assert.match(text(env), /bind "f" "\+fps_20_hold"/);

  const result = apply(env, {}, { previous: first });
  assert.deepEqual(result.keptEdited, []);
  assert.doesNotMatch(text(env), /HLSR script/);
  assert.doesNotMatch(text(env), /fps_20/);
});

test("скрипт гайда: общий alias пишется одним блоком, пустой alias — тоже блок", (t) => {
  const env = setup(t, "");
  apply(env, { g: { command: "+tau", base: "" }, h: { command: "tauswitch", base: "" } });
  const out = text(env);
  assert.equal(out.match(/HLSR script \+tau begin/g).length, 1);
  assert.ok(cfgDocument.findBlock(cfgDocument.parse(out), "_taubo"));
});

test("правленый блок: остаётся и попадает в keptEdited, с removeEdited — удаляется", (t) => {
  const env = setup(t, "");
  const first = { f: { command: "+fps_20_hold", base: "" } };
  apply(env, first);
  fs.writeFileSync(env.file, text(env).replace("fps_max 20", "fps_max 25"), "latin1");

  const kept = apply(env, {}, { previous: first });
  assert.deepEqual(kept.keptEdited, ["+fps_20_hold"]);
  assert.match(text(env), /fps_max 25/);
  assert.doesNotMatch(text(env), /bind "f"/, "бинд снят");

  const removed = apply(env, {}, { previous: first, removeEdited: true });
  assert.deepEqual(removed.keptEdited, []);
  assert.doesNotMatch(text(env), /HLSR script/);
});

test("нет заголовков и alias run: «Начать ран» на p — bind и блок run в конце файла", (t) => {
  const env = setup(t, raw('bind "w" "+forward"\r\n// моя строка\r\n'));
  apply(env, { p: { command: "run", base: "" } });
  const out = text(env);
  assert.ok(out.startsWith(raw('bind "w" "+forward"\r\n// моя строка\r\n')), "своё не тронуто");
  assert.match(out, /bind "p" "run"/);
  const block = cfgDocument.findBlock(cfgDocument.parse(out), "run");
  assert.deepEqual(block.body, [`alias run "${bindPresets.HL1_RUN_ALIAS}"`]);
  assert.ok(out.trimEnd().endsWith("// HLSR script run end"));
});

test("свой alias run игрока: блока run нет, alias не тронут", (t) => {
  const own = 'alias run "мой"\r\n';
  const env = setup(t, Buffer.from(own, "utf8").toString("latin1"));
  apply(env, { p: { command: "run", base: "" } });
  const out = text(env);
  assert.doesNotMatch(out, /HLSR script run/);
  assert.ok(out.startsWith(Buffer.from(own, "utf8").toString("latin1")));
  assert.match(out, /bind "p" "run"/);
});

test("alias run внутри блока run не считается игрока: блок обновляется, не дублируется", (t) => {
  const env = setup(t, "");
  const binds = { p: { command: "run", base: "" } };
  apply(env, binds);
  apply(env, binds, { previous: binds });
  assert.equal(text(env).match(/alias run /g).length, 1);
  assert.equal(text(env).match(/HLSR script run begin/g).length, 1);
});

test("doorsetup: блок с exec hlsr-personalization.cfg создаёт снимок из настроек, в файл игрока exec не попадает", (t) => {
  const env = setup(t, "");
  const settings = { bxt: { fov: 111 } };
  apply(env, { 3: { command: hl1Guide.scriptById("gauss-door-setup-script").actions[0].command, base: "" } }, { settings });
  const snapshot = path.join(env.dir, "hlsr-personalization.cfg");
  assert.ok(fs.existsSync(snapshot));
  assert.match(fs.readFileSync(snapshot, "latin1"), /default_fov "?111"?/);
  assert.match(text(env), /alias deffov "exec hlsr-personalization\.cfg"/);
  assert.ok(!text(env).split(/\r?\n/).some((line) => line.trim() === "exec hlsr-personalization.cfg"));
});

test("auto_alias по-прежнему в game.cfg, чужой game.cfg сохраняется", (t) => {
  const env = setup(t, "");
  fs.writeFileSync(path.join(env.dir, "game.cfg"), "echo hi\r\n", "latin1");
  const saveload = hl1Guide.scriptById("simple-saveload-script");
  const binds = { 8: { command: saveload.actions[0].command, base: "" } };
  apply(env, binds);
  apply(env, binds, { previous: binds });
  assert.equal(fs.readFileSync(path.join(env.dir, "game.cfg"), "latin1"), "echo hi\r\nauto_alias\r\n");
  assert.ok(cfgDocument.findBlock(cfgDocument.parse(text(env)), "auto_alias"));
});

test("чувствительность игрока подставляется в блок скрипта", (t) => {
  const env = setup(t, "");
  fs.writeFileSync(path.join(env.dir, "config.cfg"), 'sensitivity "2.25"\r\n', "latin1");
  const p2p = hl1Guide.scriptById("pipe2pipe-script-v2");
  apply(env, { 3: { command: p2p.actions[0].command, base: "" } });
  assert.doesNotMatch(text(env), /\{sensitivity\}/);
  assert.match(text(env), /alias \S*sens "sensitivity 2\.25"/);
});

test("Source: файл игрока autoexec.cfg, «;» пишется как SEMICOLON", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-player-binds-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const binds = { ";": { command: "+jump", base: "+use" }, RSHIFT: { command: "+speed", base: "" } };
  const result = gameBindsInstall.applyBinds(root, "ghosting-renewed", "ghostingmod", binds, EN);
  const file = path.join(root, "ghostingmod", "cfg", "autoexec.cfg");
  assert.equal(result.file, file);
  const out = fs.readFileSync(file, "utf8");
  assert.match(out, /bind "semicolon" "\+jump"/i);
  assert.match(out, /bind "rshift" "\+speed"/i);
  assert.equal(fs.existsSync(path.join(root, "ghostingmod", "cfg", "hlsr-binds.cfg")), false);
});

test("файл изменён между чтением и записью: CONFIG_CHANGED, файл не тронут", (t) => {
  const env = setup(t, "// первая\r\n");
  const fsLike = { ...fs, renameSync(...args) { return fs.renameSync(...args); } };
  // Подменяем файл после чтения, до записи: read возвращает отпечаток, write его сверяет.
  let reads = 0;
  fsLike.readFileSync = (file, ...rest) => {
    const data = fs.readFileSync(file, ...rest);
    if (String(file) === env.file && ++reads === 1) fs.writeFileSync(env.file, "// чужая правка\r\n");
    return data;
  };
  assert.throws(
    () => gameBindsInstall.applyBinds(env.root, "hl1-won", HL1, { f: { command: "+duck", base: "" } }, { ...EN, fs: fsLike }),
    (error) => error.code === "CONFIG_CHANGED"
  );
  assert.equal(fs.readFileSync(env.file, "utf8"), "// чужая правка\r\n");
});

test("правленый нужный блок не откатывается, неправленый обновляется при смене чувствительности", (t) => {
  const env = setup(t, "");
  const p2p = hl1Guide.scriptById("pipe2pipe-script-v2");
  const binds = { 3: { command: p2p.actions[0].command, base: "" }, 4: { command: "+fps_20_hold", base: "" } };
  fs.writeFileSync(path.join(env.dir, "config.cfg"), 'sensitivity "2"\r\n', "latin1");
  apply(env, binds);
  // Руками правим блок +fps_20_hold, блок pipe2pipe не трогаем.
  fs.writeFileSync(env.file, text(env).replace("fps_max 20", "fps_max 25"), "latin1");
  fs.writeFileSync(path.join(env.dir, "config.cfg"), 'sensitivity "4"\r\n', "latin1");
  const after = apply(env, { ...binds, 5: { command: "+duck", base: "" } }, { previous: binds });
  const out = text(env);
  assert.deepEqual(after.keptEdited, []);
  assert.match(out, /fps_max 25/, "правка в блоке активного скрипта на месте");
  assert.match(out, /alias \S*sens "sensitivity 4"/, "неправленый блок обновлён");
  assert.doesNotMatch(out, /sensitivity 2"/);
});

// R15: пишется только разница с прежними переназначениями.
const OVERRIDE = { f: { command: "+jump", base: "+use" } };

function withGameConfig(t, own) {
  const env = setup(t, own);
  fs.writeFileSync(path.join(env.dir, "config.cfg"), 'bind f "+use"\r\nexec userconfig.cfg\r\n', "latin1");
  return env;
}

test("R15: бинд, правленный руками после переназначения, следующее «Сохранить» не откатывает (проба 3)", (t) => {
  const env = withGameConfig(t, "// ===== HLSR: binds =====\r\n");
  apply(env, OVERRIDE, { previous: {} });
  fs.writeFileSync(env.file, text(env).replace('"+jump"', '"+duck" // my edit'), "latin1");
  const edited = text(env);
  // Окно перечитало файл, игрок поменял другую клавишу.
  apply(env, { ...OVERRIDE, g: { command: "+use", base: "" } }, { previous: OVERRIDE });
  assert.ok(text(env).includes('bind "f" "+duck" // my edit'), "правка игрока на месте");
  assert.equal(text(env), edited.replace('bind "f" "+duck" // my edit\r\n', 'bind "f" "+duck" // my edit\r\nbind "g" "+use"\r\n'));
  // Сохранение без изменений в биндах (например, из «Персонализации») файл не трогает.
  apply(env, OVERRIDE, { previous: OVERRIDE });
  assert.ok(text(env).includes('"+duck" // my edit'));
});

test("R15: «Сбросить клавишу», потом два сохранения — строка игрока остаётся (проба 1B)", (t) => {
  const env = withGameConfig(t, '// ===== HLSR: binds =====\r\n\r\n// ===== Your own =====\r\nbind f "+use" // my use\r\n');
  apply(env, OVERRIDE, { previous: {} });
  assert.ok(text(env).includes('bind f "+jump" // my use'));
  const revert = { f: { command: "+use", base: "+use" } };
  apply(env, revert, { previous: OVERRIDE });
  const reverted = text(env);
  assert.ok(reverted.includes('bind f "+use" // my use'));
  // Следующее открытие: возврат игре уже не нужен, окно его не показывает (pruneOverrides).
  const fileBinds = gameBindsInstall.readInstallBinds(env.root, HL1, "goldsrc", EN).binds;
  const draft = require("../src/shared/game-binds").pruneOverrides(revert, fileBinds);
  assert.deepEqual(draft, {});
  apply(env, draft, { previous: revert });
  assert.equal(text(env), reverted, "пропавший возврат файл не трогает");
});

test("R15: «Сбросить» всё окно — возвраты пишутся один раз, следующее сохранение строки не удаляет", (t) => {
  const env = withGameConfig(t, "// ===== HLSR: binds =====\r\n");
  const overrides = { f: { command: "+jump", base: "+use" }, g: { command: "+duck", base: "" } };
  apply(env, overrides, { previous: {} });
  const reset = require("../src/shared/game-binds").resetOverrides(overrides);
  apply(env, reset, { previous: overrides });
  const after = text(env);
  assert.match(after, /bind "f" "\+use"/);
  assert.match(after, /unbind "g"/);
  apply(env, {}, { previous: reset });
  assert.equal(text(env), after);
});

test("R15: импорт поверх переназначения — следующее сохранение не возвращает старый бинд (проба 2D)", (t) => {
  const env = withGameConfig(t, '// ===== HLSR: binds =====\r\nbind "f" "+jump"\r\n');
  const cfgImport = require("../src/shared/cfg-import");
  const built = cfgImport.flattenImport({ engine: "goldsrc", root: { name: "my.cfg", text: 'bind f "+attack2"\r\nbind g "+duck"\r\n' }, files: {}, missing: [] });
  assert.equal(gameBindsInstall.importPlayerConfig(env.root, "hl1-won", HL1, built, EN).status, "ok");
  const fileBinds = gameBindsInstall.readInstallBinds(env.root, HL1, "goldsrc", EN).binds;
  const draft = require("../src/shared/game-binds").pruneOverrides(OVERRIDE, fileBinds);
  apply(env, draft, { previous: OVERRIDE });
  assert.match(text(env), /bind f "\+attack2"/);
  assert.doesNotMatch(text(env), /\+jump/);
});

test("C2/I5: импорт config.cfg с unbindall — правила и настройки шаблона выше, новые бинды и «Начать ран» после импорта работают (проба 5)", (t) => {
  const env = setup(t);
  fs.writeFileSync(path.join(env.dir, "config.cfg"), 'unbindall\r\nbind w "+forward"\r\nexec userconfig.cfg\r\n', "latin1");
  const cfgImport = require("../src/shared/cfg-import");
  const imported = { name: "config.cfg", text: 'unbindall\r\nbind w "+forward"\r\nbind space "+jump"\r\nsensitivity "2"\r\ncl_showfps "0"\r\n' };
  const built = cfgImport.flattenImport({ engine: "goldsrc", root: imported, files: {}, missing: [] });
  assert.equal(gameBindsInstall.importPlayerConfig(env.root, "hl1-won", HL1, built, EN).status, "ok");
  const head = playerConfig.templateLines("hl1-won", ["rules", "settings"]).join("\r\n") + "\r\n\r\n";
  const body = built.text.replace(/\r?\n/g, "\r\n");
  assert.equal(text(env), head + body, "правила и настройки шаблона, затем импорт как есть");
  assert.doesNotMatch(text(env), /Your own/);

  apply(env, { k: { command: "+duck", base: "" }, p: { command: "run", base: "" } }, { previous: {} });
  const out = text(env);
  assert.ok(out.startsWith(head + body), "импорт не тронут, разделы лаунчера дописаны после него");
  const { runChain } = require("./lib/cfg-state");
  const state = runChain((name) => {
    try { return fs.readFileSync(path.join(env.dir, name), "latin1"); } catch (error) { return null; }
  }, ["config.cfg"]);
  assert.equal(state.binds.get("k"), "+duck", "новый бинд после unbindall импорта");
  assert.equal(state.binds.get("p"), "run", "«Начать ран» не правит строку шаблона выше unbindall");
  assert.equal(state.aliases.get("run"), bindPresets.HL1_RUN_ALIAS, "alias run из шаблона");
  assert.equal(state.cvars.get("_bxt_save_runtime_data_in_demos"), "1");
  assert.equal(state.cvars.get("cl_showfps"), "0", "что импорт задаёт сам, перекрывает шаблон");
  assert.match(out.split(/\r?\n/).find((line) => line.trim() && !line.startsWith("//")), /^exec WON\.cfg\b/, "правила первой командой");
});

// Окно настроек через настоящий index.js: хранилище, импорт, сохранение.
function controllerFor(root) {
  const GameDelivery = require("../src/main/game-delivery");
  const GameSettingsStore = require("../src/main/game-delivery/GameSettingsStore");
  const store = new GameSettingsStore({ file: path.join(root, "game-settings.json") });
  const controller = Object.create(GameDelivery.prototype);
  controller.userDataPath = root;
  controller.ensureSettingsStore = () => store;
  controller.resolveGameId = (id) => id;
  controller.gameLanguages = () => [];
  controller.service = { installService: { readInstalled: () => ({ installPath: root }) }, isGameRunning: () => false };
  return { store, controller };
}

test("C1/N1: импорт снимает из хранилища все переназначения; сохранение после импорта строку импорта не трогает и старое не пишет (проба 2E)", (t) => {
  const env = withGameConfig(t, "// ===== HLSR: binds =====\r\n");
  const { store, controller } = controllerFor(env.root);
  const overrides = { ...OVERRIDE, k: { command: "+use", base: "" } };
  assert.equal(controller.gameSettings("save", "hl1-won", { binds: overrides }).status, "ok");
  assert.match(text(env), /bind "f" "\+jump"/);

  const imported = controller.importPlayerConfig("hl1-won", { root: { name: "my.cfg", text: 'bind f "+attack2"\r\nbind g "+duck"\r\n' } });
  assert.equal(imported.status, "ok");
  assert.deepEqual(Object.keys(store.get("hl1-won").binds), [], "файл главный: переназначения сняты, в том числе k, которого импорт не касается");

  // Окно клавиатуры убрало f из черновика (adoptImport), «Сохранить» — строка импорта на месте.
  const opened = controller.gameSettings("get", "hl1-won");
  assert.deepEqual(opened.settings.binds, {}, "окно открывается с очищенными переназначениями");
  const draft = { ...opened.settings };
  assert.equal(controller.gameSettings("save", "hl1-won", draft, undefined, { configSha256: opened.configSha256 }).status, "ok");
  assert.match(text(env), /bind f "\+attack2"/);
  assert.doesNotMatch(text(env), /\+jump/);
});
test("N1/N4 (проба rr-probe1): после импорта с unbindall клавиши k и m берутся из файла, сохранение старое не пишет", (t) => {
  const env = withGameConfig(t, "// ===== HLSR: binds =====\r\n");
  const { store, controller } = controllerFor(env.root);
  const { runChain } = require("./lib/cfg-state");
  const chain = () => runChain((name) => {
    try { return fs.readFileSync(path.join(env.dir, name), "latin1"); } catch (error) { return null; }
  }, ["config.cfg"]);
  const overrides = { k: { command: "+duck", base: "" }, m: { command: "run", base: "" } };
  assert.equal(controller.gameSettings("save", "hl1-won", { binds: overrides }).status, "ok");
  assert.equal(chain().binds.get("k"), "+duck");

  const imported = controller.importPlayerConfig("hl1-won", { root: { name: "config.cfg", text: 'unbindall\r\nbind w "+forward"\r\n' } });
  assert.equal(imported.status, "ok");
  assert.deepEqual(store.get("hl1-won").binds, {});
  const opened = controller.gameSettings("get", "hl1-won");
  assert.deepEqual(opened.settings.binds, {}, "«Клавиатура» и «Скрипты» показывают файл");
  assert.equal(opened.keyBinds.binds.k, undefined);
  assert.equal(controller.gameSettings("save", "hl1-won", opened.settings, undefined, { configSha256: opened.configSha256 }).status, "ok");
  const state = chain();
  assert.equal(state.binds.get("k"), undefined);
  assert.equal(state.binds.get("m"), undefined);
  assert.equal(state.binds.get("w"), "+forward");

  // «Вернуть прежний»: файл прежний, переназначения не воскресают (R23).
  assert.equal(controller.undoPlayerConfigImport("hl1-won").status, "ok");
  assert.deepEqual(store.get("hl1-won").binds, {});
  assert.equal(chain().binds.get("k"), "+duck", "прежние бинды лаунчера уже в восстановленном файле");
});

test("чужой блок HLSR script, о котором лаунчер не знает: молча остаётся, вопроса нет", (t) => {
  const own = "// HLSR script x begin\r\nalias x \"echo hi\"\r\n// HLSR script x end\r\n";
  const env = setup(t, own);
  const result = apply(env, {}, { previous: { f: { command: "+duck", base: "" } } });
  assert.deepEqual(result.keptEdited, []);
  assert.equal(text(env), own);
  assert.deepEqual(apply(env, {}, { previous: {}, removeEdited: true }).keptEdited, []);
  assert.equal(text(env), own, "и removeEdited чужой блок не удаляет");
});