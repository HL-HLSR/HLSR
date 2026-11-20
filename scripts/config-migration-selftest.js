"use strict";

// Переход с 0.4.8 в один файл игрока (§9 замысла). Главное условие — итог в игре
// тот же: цепочка 0.4.8 и новый файл выполняются эмулятором cfg (scripts/lib/cfg-state.js),
// итоговые переменные, бинды и alias сравниваются. Файлы hlsr-*.cfg строятся генераторами
// 0.4.8 (договор R1), как их пишет 0.4.8 перед запуском, а не берутся с диска.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const { runChain } = require("./lib/cfg-state");
const configMigration = require("../src/main/game-delivery/config-migration");
const playerConfig = require("../src/main/game-delivery/player-config");
const cfgSearch = require("../src/main/game-delivery/cfg-search");
const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const goldsrcPersonalization = require("../src/main/game-delivery/goldsrc-personalization");
const ghostingPersonalization = require("../src/main/game-delivery/ghosting-personalization");
const hl1Default = require("../src/main/game-delivery/hl1-default-userconfig");
const ghostingDefault = require("../src/main/game-delivery/ghosting-default-autoexec");
const launchArgs = require("../src/main/game-delivery/launch-args");
const cfgDocument = require("../src/shared/cfg-document");
const gameBinds = require("../src/shared/game-binds");
const gameConfig = require("../src/shared/game-config");
const bindPresets = require("../src/shared/bind-presets");

const temp = () => fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-config-migration-"));
const HL1 = "Half-Life/valve_WON";
const GAME_DIRS = { "hl1-won": HL1, "ghosting-renewed": "ghostingmod", "hl2-2707": "hl2" };
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };
const NOW = new Date(2026, 10, 6, 12, 34, 56);
const STAMP = "migration-20261106-123456";
const VERSION = "0.4.9";
const MARK = gameBindsInstall.IMPORT_TRAILER_MARK;
const LAUNCHER_EXECS = "exec hlsr-personalization.cfg\r\nexec hlsr-binds.cfg\r\n";

const engineOf = (gameId) => bindPresets.engineFor(gameId);
const encodingOf = (gameId) => gameBinds.engineOf(engineOf(gameId)).encoding;
const modDir = (root, gameId) => cfgSearch.modDirOf(root, GAME_DIRS[gameId], engineOf(gameId));

// config.cfg, который пишет игра: последней строкой — exec userconfig.cfg.
const HL1_CONFIG = [
  "unbindall", 'bind "w" "+forward"', 'bind "h" "+use"', 'bind "p" "+attack"',
  'sensitivity "2.5"', 'default_fov "90"', 'fps_max "72"', "exec userconfig.cfg", "",
].join("\r\n");
const WON = "fps_max 100\r\nclockwindow 0\r\n";

function put(root, gameId, name, text) {
  const file = path.join(modDir(root, gameId), ...name.split("/"));
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, Buffer.from(text, encodingOf(gameId)));
}

// Что 0.4.8 пишет перед запуском: персонализация и бинды из её генераторов.
function launcherFiles(root, gameId, settings) {
  const engine = engineOf(gameId);
  const clean = gameBinds.sanitizeOverrides(settings.binds);
  const { aliases } = gameBindsInstall.scriptAliases(root, gameId, GAME_DIRS[gameId], engine, clean, fs);
  const files = { "hlsr-binds.cfg": gameBinds.toCfgLines(settings.binds, { engine, aliases }) };
  if (gameId === "hl1-won") files["hlsr-personalization.cfg"] = goldsrcPersonalization.toCfgLines(settings);
  if (gameId === "ghosting-renewed")
    files["hlsr-personalization.cfg"] = ghostingPersonalization.toCfgLines(
      ghostingPersonalization.sanitizePersonalization(settings.personalization));
  return files;
}

function install(gameId, files, settings, options = {}) {
  const root = temp();
  for (const [name, text] of Object.entries(files)) put(root, gameId, name, text);
  const generated = launcherFiles(root, gameId, settings);
  for (const name of options.generated || Object.keys(generated)) put(root, gameId, name, generated[name]);
  if (options.record) {
    fs.mkdirSync(path.join(root, ".hlsr"), { recursive: true });
    fs.writeFileSync(path.join(root, ".hlsr", "cfg-import.json"), JSON.stringify(options.record, null, 2));
  }
  return root;
}

// Чтение cfg как в движке: папки поиска, имя без регистра, подпапки мода.
function reader(root, gameId, language) {
  const files = cfgSearch.filesOf(root, GAME_DIRS[gameId], engineOf(gameId), language);
  return (name) => {
    let file = files.modDir;
    if (name.includes("/")) {
      for (const part of name.split("/")) {
        const hit = fs.existsSync(file) ? fs.readdirSync(file).find((item) => item.toLowerCase() === part) : null;
        if (!hit) return null;
        file = path.join(file, hit);
      }
    } else if (files.exists(name)) {
      file = files.pathOf(name);
    } else {
      return null;
    }
    return fs.readFileSync(file, encodingOf(gameId));
  };
}

function state(root, gameId, language = EN) {
  const roots = gameId === "hl1-won" ? ["config.cfg"] : gameConfig.ENGINE_ROOTS[engineOf(gameId)];
  return runChain(reader(root, gameId, language), roots);
}

function assertSame(before, after) {
  assert.deepEqual(after.cvars, before.cvars);
  assert.deepEqual(after.binds, before.binds);
  assert.deepEqual(after.aliases, before.aliases);
}

const migrate = (root, gameId, settings, extra = {}) =>
  configMigration.migrate(root, gameId, GAME_DIRS[gameId], { ...EN, settings, launcherVersion: VERSION, now: NOW, ...extra });

const textOf = (root, gameId) => fs.readFileSync(playerConfig.locate(root, gameId, GAME_DIRS[gameId], EN).file, encodingOf(gameId));
const count = (text, needle) => text.split(needle).length - 1;
const runAliases = (text) => text.split(/\r?\n/)
  .filter((line) => gameConfig.lineCommands(line).some((command) => cfgDocument.keyOf(command.args) === "alias:run")).length;
const launcherLeft = (root, gameId) => fs.readdirSync(modDir(root, gameId)).filter((name) => /^hlsr-.*\.cfg$/i.test(name)).sort();

function listFiles(dir, prefix = "") {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => entry.isDirectory()
    ? listFiles(path.join(dir, entry.name), `${prefix}${entry.name}/`)
    : [`${prefix}${entry.name}`]).sort();
}

function hl1Settings() {
  const settings = launchArgs.defaultSettings();
  settings.bxt = Object.assign({}, settings.bxt, { fov: 100, timer: true });
  settings.personalization = Object.assign({}, settings.personalization, { crossEnabled: true });
  // run — alias run и в hlsr-binds.cfg; AMEL — скрипт гайда с чувствительностью игрока из config.cfg.
  settings.binds = { f: { command: "run", base: "" }, k: { command: "AMEL", base: "" }, h: { command: "", base: "+use" } };
  return settings;
}

function cleanHl1(settings) {
  return install("hl1-won", { "config.cfg": HL1_CONFIG, "WON.cfg": WON, "userconfig.cfg": hl1Default.TEXT + LAUNCHER_EXECS }, settings);
}

test("runChain: exec без регистра, без повторов, до глубины 8; bind, unbind, unbindall, alias, cvar — последнее", () => {
  const files = {
    "config.cfg": [
      'bind "a" "x"', "unbindall", 'bind B "+jump"', "bind c y", "unbind c", "fov 1", 'fov "2" // комментарий',
      'alias go "say 1; say 2"', "EXEC Next.cfg", "volume 1", "exec next.cfg", "exec d1.cfg", "",
    ].join("\r\n"),
    "next.cfg": "volume 0.5\n",
  };
  for (let i = 1; i <= 9; i += 1) files[`d${i}.cfg`] = `depth ${i}\nexec d${i + 1}.cfg\n`;
  const result = runChain((name) => (name in files ? files[name] : null), ["config.cfg"]);
  assert.deepEqual(result.binds, new Map([["b", "+jump"]]));
  assert.deepEqual(result.aliases, new Map([["go", "say 1; say 2"]]));
  assert.equal(result.cvars.get("fov"), "2");
  assert.equal(result.cvars.get("volume"), "1");
  // config.cfg — глубина 0, d1.cfg — 1, d8.cfg — 8, d9.cfg уже не выполняется.
  assert.equal(result.cvars.get("depth"), "8");
});

test("HL1 после первого запуска 0.4.8: итог тот же, hlsr-*.cfg в копии, alias run один, отметка", () => {
  const settings = hl1Settings();
  const root = cleanHl1(settings);
  const before = state(root, "hl1-won");
  assert.equal(before.cvars.get("fps_max"), "100");
  assert.equal(before.cvars.get("default_fov"), "100");
  assert.equal(before.binds.get("f"), "run");
  assert.equal(before.binds.has("h"), false);
  assert.equal(before.aliases.get("amel_sens"), "sensitivity 2.5");

  const result = migrate(root, "hl1-won", settings);
  assert.equal(result.status, "migrated");
  assertSame(before, state(root, "hl1-won"));

  const text = textOf(root, "hl1-won");
  assert.equal(runAliases(text), 1);
  assert.equal(count(text, cfgDocument.SECTIONS.personalization), 1);
  assert.equal(count(text, cfgDocument.SECTIONS.binds), 1);
  assert.doesNotMatch(text, /exec\s+hlsr-/i);
  assert.doesNotMatch(text, /Generated by HLSR/);
  assert.match(text, /^[\x00-\x7f]*$/);
  assert.deepEqual(launcherLeft(root, "hl1-won"), []);
  assert.equal(result.backupDir, path.join(root, ".hlsr", "backup", STAMP));
  assert.deepEqual(listFiles(result.backupDir), [
    "Half-Life/valve_WON/hlsr-binds.cfg", "Half-Life/valve_WON/hlsr-personalization.cfg", "Half-Life/valve_WON/userconfig.cfg",
  ]);
  assert.equal(fs.readFileSync(path.join(result.backupDir, "Half-Life", "valve_WON", "userconfig.cfg"), "latin1"),
    hl1Default.TEXT + LAUNCHER_EXECS);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(root, ".hlsr", "config-setup.json"), "utf8")), { migrated: VERSION });
  assert.equal(configMigration.needsMigration(root, "hl1-won", HL1, EN), false);
});

test("HL1: повторный переход — skipped, файл байт в байт тот же", () => {
  const settings = hl1Settings();
  const root = cleanHl1(settings);
  const file = migrate(root, "hl1-won", settings).file;
  const bytes = fs.readFileSync(file);
  const again = migrate(root, "hl1-won", settings, { now: new Date(2026, 10, 7) });
  assert.equal(again.status, "skipped");
  assert.deepEqual(fs.readFileSync(file), bytes);
  assert.deepEqual(fs.readdirSync(path.join(root, ".hlsr", "backup")), [STAMP]);
});

test("HL1: импорт из трёх файлов с хвостом — файлы записи раскрыты на месте, хвост выброшен", () => {
  const settings = hl1Settings();
  const body = ['bind "q" "+duck"', 'default_fov "110"', "exec b.cfg", 'alias +x "exec c.cfg"'].join("\r\n");
  const record = {
    version: 1,
    files: [{ path: "scripts/a.cfg", created: true }, { path: "b.cfg", created: true }, { path: "c.cfg", created: true }],
    execs: ["scripts/a.cfg"],
  };
  const root = install("hl1-won", {
    "config.cfg": HL1_CONFIG,
    "WON.cfg": WON,
    "userconfig.cfg": hl1Default.TEXT + 'exec "scripts/a.cfg"\r\n' + LAUNCHER_EXECS,
    // Хвост — как его пишет syncImportTrailer 0.4.8.
    "scripts/a.cfg": [body, "", MARK, "exec hlsr-personalization.cfg", "exec hlsr-binds.cfg"].join("\r\n") + "\r\n",
    "b.cfg": 'bind "e" "+use"\r\nhud_fastswitch "1"\r\n',
    "c.cfg": "fps_max 50\r\n",
  }, settings, { record });
  const before = state(root, "hl1-won");
  assert.equal(before.binds.get("q"), "+duck");
  assert.equal(before.cvars.get("hud_fastswitch"), "1");
  assert.equal(before.aliases.get("+x"), "exec c.cfg");
  assert.equal(before.cvars.get("default_fov"), "100");

  assert.equal(migrate(root, "hl1-won", settings).status, "migrated");
  assertSame(before, state(root, "hl1-won"));
  const text = textOf(root, "hl1-won");
  assert.ok(text.includes("// ===== from scripts/a.cfg =====\r\n") && text.includes("// ===== from b.cfg =====\r\n"));
  assert.ok(text.indexOf("// ===== from b.cfg =====") < text.indexOf(cfgDocument.SECTIONS.personalization));
  assert.ok(!text.includes(MARK));
  assert.doesNotMatch(text, /^exec\s+"?(scripts\/a|b)\.cfg/im);
  assert.ok(text.includes('alias +x "exec c.cfg"'));
  assert.equal(runAliases(text), 1);
  assert.equal(count(text, cfgDocument.SECTIONS.personalization), 1);
  for (const name of ["scripts/a.cfg", "b.cfg", "c.cfg"]) assert.ok(fs.existsSync(path.join(modDir(root, "hl1-won"), name)), name);
  assert.ok(fs.existsSync(path.join(root, ".hlsr", "cfg-import.json")));
  assert.deepEqual(launcherLeft(root, "hl1-won"), []);
});

test("HL1: правленый userconfig (default_fov 110 выше exec) — итог по-прежнему из персонализации, строки игрока целы", () => {
  const settings = hl1Settings();
  delete settings.binds.f;
  // Комментарий в cp1251: байты 0x80–0xFF переходят без изменений (latin1).
  const own = "// Ïðèâåò \u0080ÿ\r\n" + hl1Default.TEXT + "default_fov 110\r\n";
  const root = install("hl1-won", { "config.cfg": HL1_CONFIG, "WON.cfg": WON, "userconfig.cfg": own + LAUNCHER_EXECS }, settings);
  const before = state(root, "hl1-won");
  assert.equal(before.cvars.get("default_fov"), "100");

  migrate(root, "hl1-won", settings);
  const after = state(root, "hl1-won");
  assertSame(before, after);
  assert.equal(after.cvars.get("default_fov"), "100");
  const file = playerConfig.locate(root, "hl1-won", HL1, EN).file;
  // alias run шаблона ниже не перекрыт — остаётся; весь текст игрока — те же байты.
  assert.deepEqual(fs.readFileSync(file).subarray(0, own.length), Buffer.from(own, "latin1"));
  assert.equal(runAliases(textOf(root, "hl1-won")), 1);
});

test("HL1: стоковый userconfig и только exec hlsr-binds.cfg (не запускали) — как сделала бы 0.4.8 при запуске", () => {
  const settings = hl1Settings();
  const stock = hl1Default.STOCK.replace(/\n/g, "\r\n") + "\r\nexec hlsr-binds.cfg\r\n";
  const root = install("hl1-won", { "config.cfg": HL1_CONFIG, "WON.cfg": WON, "userconfig.cfg": stock }, settings,
    { generated: ["hlsr-binds.cfg"] });
  // 0.4.8 перед запуском: стоковый файл → стандартный со строками exec (ensureDefaultUserconfig),
  // exec hlsr-personalization.cfg дописан в конец (ensureExecLine), персонализация записана.
  const launched = install("hl1-won", {
    "config.cfg": HL1_CONFIG,
    "WON.cfg": WON,
    "userconfig.cfg": hl1Default.TEXT + "exec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n",
  }, settings);
  const before = state(launched, "hl1-won");

  migrate(root, "hl1-won", settings);
  assertSame(before, state(root, "hl1-won"));
  const text = textOf(root, "hl1-won");
  assert.ok(text.startsWith(hl1Default.TEXT.slice(0, hl1Default.TEXT.indexOf("alias run"))));
  assert.ok(text.indexOf(cfgDocument.SECTIONS.binds) < text.indexOf(cfgDocument.SECTIONS.personalization));
  assert.equal(runAliases(text), 1);
});

test("HL1 с valve_WON_russian: config.cfg в языковой папке, файлы 0.4.8 в valve_WON — итог тот же", () => {
  const RU = { language: "russian" };
  const settings = hl1Settings();
  // Без скриптов гайда: их alias 0.4.8 строит по языку Steam этой машины.
  settings.binds = { f: { command: "run", base: "" } };
  const root = install("hl1-won", { "WON.cfg": WON, "userconfig.cfg": hl1Default.TEXT + LAUNCHER_EXECS }, settings);
  const russian = path.join(root, "Half-Life", "valve_WON_russian");
  fs.mkdirSync(russian);
  fs.writeFileSync(path.join(russian, "config.cfg"), HL1_CONFIG, "latin1");
  const before = state(root, "hl1-won", RU);
  assert.equal(before.cvars.get("sensitivity"), "2.5");
  assert.equal(before.cvars.get("default_fov"), "100");

  const result = migrate(root, "hl1-won", settings, RU);
  assert.equal(result.file, path.join(root, "Half-Life", "valve_WON", "userconfig.cfg"));
  assertSame(before, state(root, "hl1-won", RU));
  assert.deepEqual(launcherLeft(root, "hl1-won"), []);
  assert.deepEqual(fs.readdirSync(russian), ["config.cfg"]);
});

test("HL1: импорт 0.4.7 (hlsr-import.cfg) — раскрыт; файл, который зовёт alias, остаётся на месте", () => {
  const settings = hl1Settings();
  const legacy = [
    "// Imported by HLSR Launcher (game settings, Keyboard). Remove it there.",
    'bind "q" "+duck"', "exec hlsr-import-jump.cfg", 'alias +t "exec hlsr-import-tog.cfg"', 'bind "t" "+t"',
    "", MARK, "exec hlsr-personalization.cfg", "exec hlsr-binds.cfg",
  ].join("\r\n") + "\r\n";
  const root = install("hl1-won", {
    "config.cfg": HL1_CONFIG,
    "WON.cfg": WON,
    "userconfig.cfg": hl1Default.TEXT + "exec hlsr-import.cfg\r\n" + LAUNCHER_EXECS,
    "hlsr-import.cfg": legacy,
    "hlsr-import-jump.cfg": 'bind "space" "+jump"\r\ndefault_fov "120"\r\n',
    "hlsr-import-tog.cfg": "fps_max 99\r\n",
  }, settings);
  const before = state(root, "hl1-won");
  assert.equal(before.binds.get("space"), "+jump");

  const result = migrate(root, "hl1-won", settings);
  assertSame(before, state(root, "hl1-won"));
  const text = textOf(root, "hl1-won");
  assert.ok(text.includes("// ===== from hlsr-import.cfg =====") && text.includes("// ===== from hlsr-import-jump.cfg ====="));
  assert.ok(!text.includes(MARK));
  assert.deepEqual(launcherLeft(root, "hl1-won"), ["hlsr-import-tog.cfg"]);
  assert.ok(fs.existsSync(path.join(result.backupDir, "Half-Life", "valve_WON", "hlsr-import.cfg")));
  assert.ok(fs.existsSync(path.join(result.backupDir, "Half-Life", "valve_WON", "hlsr-import-jump.cfg")));
});

test("Ghosting с персонализацией: итог тот же, autoexec игрока в UTF-8 цел", () => {
  const settings = launchArgs.defaultSettings();
  settings.personalization = Object.assign(ghostingPersonalization.defaultPersonalization(), {
    crossEnabled: true, hudEnabled: true, hudColor: { r: 0, g: 255, b: 255 },
  });
  settings.binds = { j: { command: "+jump", base: "" }, mouse4: { command: "", base: "save quick" } };
  const root = install("ghosting-renewed", {
    "config.cfg": 'unbindall\nbind "SPACE" "+jump"\ngh_cross "0"\nfps_max "300"\n',
    "autoexec.cfg": ghostingDefault + "exec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n",
  }, settings);
  const before = state(root, "ghosting-renewed");
  assert.equal(before.cvars.get("gh_cross"), "1");
  assert.equal(before.binds.has("mouse4"), false);

  assert.equal(migrate(root, "ghosting-renewed", settings).status, "migrated");
  assertSame(before, state(root, "ghosting-renewed"));
  const file = playerConfig.locate(root, "ghosting-renewed", "ghostingmod", EN).file;
  const own = Buffer.from(ghostingDefault, "utf8");
  assert.deepEqual(fs.readFileSync(file).subarray(0, own.length), own);
  const text = textOf(root, "ghosting-renewed");
  assert.doesNotMatch(text, /exec\s+hlsr-/i);
  assert.ok(text.indexOf(cfgDocument.SECTIONS.binds) < text.indexOf(cfgDocument.SECTIONS.personalization));
  assert.deepEqual(launcherLeft(root, "ghosting-renewed"), []);
});

test("C3: Ghosting, autoexec в cp1251 и gh_name кириллицей — байты игрока целы, gh_name в UTF-8, как писала 0.4.8", () => {
  const cp1251 = fs.readFileSync(path.join(__dirname, "fixtures", "single-config", "cp1251-autoexec.cfg"));
  const settings = launchArgs.defaultSettings();
  settings.personalization = Object.assign(ghostingPersonalization.defaultPersonalization(), { crossEnabled: true });
  settings.personalization.extra = Object.assign({}, settings.personalization.extra, { gh_name: "Влад" });
  settings.binds = { j: { command: "+jump", base: "" } };
  const root = install("ghosting-renewed", { "config.cfg": 'bind "w" "+forward"\n' }, settings);
  const file = playerConfig.locate(root, "ghosting-renewed", "ghostingmod", EN).file;
  fs.writeFileSync(file, Buffer.concat([cp1251, Buffer.from(LAUNCHER_EXECS, "latin1")]));
  const before = state(root, "ghosting-renewed");
  assert.equal(before.cvars.get("gh_name"), "Влад");

  assert.equal(migrate(root, "ghosting-renewed", settings).status, "migrated");
  assertSame(before, state(root, "ghosting-renewed"));
  const migrated = fs.readFileSync(file);
  assert.deepEqual(migrated.subarray(0, cp1251.length), cp1251, "строки игрока cp1251 байт в байт");
  assert.ok(migrated.includes(Buffer.from('gh_name "Влад"', "utf8")), "gh_name — байты UTF-8");
  const shown = playerConfig.readPersonalization(root, "ghosting-renewed", "ghostingmod", launchArgs.defaultSettings(), EN);
  assert.equal(shown.personalization.extra.gh_name, "Влад", "окно показывает имя, а не байты");
});

test("HL2 2707 с биндами и hlsr-config.cfg: итог тот же", () => {
  const settings = launchArgs.defaultSettings();
  settings.binds = { v: { command: "noclip", base: "" }, e: { command: "", base: "+use" } };
  const root = install("hl2-2707", {
    "config.cfg": 'unbindall\nbind "e" "+use"\nbind "w" "+forward"\nfps_max "300"\n',
    "autoexec.cfg": "cl_showpos 1\r\nexec hlsr-binds.cfg\r\nexec hlsr-config.cfg\r\n",
    "hlsr-config.cfg": '// HLSR Launcher: game settings from the Configs tab. Executed from autoexec.cfg.\nfps_max "200"\n',
  }, settings);
  const before = state(root, "hl2-2707");
  assert.equal(before.cvars.get("fps_max"), "200");
  assert.equal(before.binds.get("v"), "noclip");
  assert.equal(before.binds.has("e"), false);

  const result = migrate(root, "hl2-2707", settings);
  assertSame(before, state(root, "hl2-2707"));
  const text = textOf(root, "hl2-2707");
  assert.ok(text.startsWith("cl_showpos 1\r\n"));
  assert.ok(text.includes(cfgDocument.SECTIONS.binds) && text.includes(cfgDocument.SECTIONS.settings));
  assert.doesNotMatch(text, /exec\s+hlsr-|HLSR Launcher:/i);
  assert.deepEqual(launcherLeft(root, "hl2-2707"), []);
  assert.deepEqual(listFiles(result.backupDir), ["hl2/cfg/autoexec.cfg", "hl2/cfg/hlsr-binds.cfg", "hl2/cfg/hlsr-config.cfg"]);
});

test("переход прерван после записи нового файла — второй вызов доводит до конца без второго раздела", () => {
  const settings = hl1Settings();
  const root = cleanHl1(settings);
  const before = state(root, "hl1-won");
  const backupRoot = path.join(root, ".hlsr", "backup");
  const failing = {
    ...fs,
    renameSync: (from, to) => {
      if (String(to).startsWith(backupRoot)) throw Object.assign(new Error("EBUSY: файл занят"), { code: "EBUSY" });
      return fs.renameSync(from, to);
    },
  };
  assert.throws(() => migrate(root, "hl1-won", settings, { fs: failing }), /EBUSY/);
  assert.equal(count(textOf(root, "hl1-won"), cfgDocument.SECTIONS.personalization), 1);
  assert.deepEqual(launcherLeft(root, "hl1-won"), ["hlsr-binds.cfg", "hlsr-personalization.cfg"]);
  assert.equal(fs.existsSync(path.join(root, ".hlsr", "config-setup.json")), false);
  assert.equal(configMigration.needsMigration(root, "hl1-won", HL1, EN), true);

  const result = migrate(root, "hl1-won", settings, { now: new Date(2026, 10, 6, 12, 40, 0) });
  assert.equal(result.status, "migrated");
  const text = textOf(root, "hl1-won");
  assert.equal(count(text, cfgDocument.SECTIONS.personalization), 1);
  assert.equal(count(text, cfgDocument.SECTIONS.binds), 1);
  assert.equal(runAliases(text), 1);
  assertSame(before, state(root, "hl1-won"));
  assert.deepEqual(launcherLeft(root, "hl1-won"), []);
  // Копия прежнего файла игрока — из первого, прерванного вызова: та же папка.
  assert.equal(result.backupDir, path.join(backupRoot, STAMP));
  assert.equal(fs.readFileSync(path.join(result.backupDir, "Half-Life", "valve_WON", "userconfig.cfg"), "latin1"),
    hl1Default.TEXT + LAUNCHER_EXECS);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(root, ".hlsr", "config-setup.json"), "utf8")), { migrated: VERSION });
});

test("ensure успел заменить файл 0.4.8 шаблоном до перехода — exec 0.4.8 дописаны в конец и раскрыты, итог тот же", () => {
  const settings = hl1Settings();
  const root = cleanHl1(settings);
  const before = state(root, "hl1-won");
  assert.equal(playerConfig.ensure(root, "hl1-won", HL1, EN).created, true);
  assert.equal(migrate(root, "hl1-won", settings).status, "migrated");
  assertSame(before, state(root, "hl1-won"));
  assert.equal(runAliases(textOf(root, "hl1-won")), 1);
});

test("HL1: скрипт doorsetup зовёт hlsr-personalization.cfg — файл остаётся снимком, переписан из settings", () => {
  const old = hl1Settings();
  old.bxt = Object.assign({}, old.bxt, { fov: 90 });
  old.binds = { d: { command: "+doorsetup", base: "" } };
  // Настройки новее файла на диске: персонализацию HL1 0.4.8 пишет только при запуске.
  const settings = hl1Settings();
  settings.bxt = Object.assign({}, settings.bxt, { fov: 110 });
  settings.binds = old.binds;
  const files = { "config.cfg": HL1_CONFIG, "WON.cfg": WON, "userconfig.cfg": hl1Default.TEXT + LAUNCHER_EXECS };
  const root = install("hl1-won", files, old);
  const launched = install("hl1-won", files, settings);
  const before = state(launched, "hl1-won");
  assert.equal(before.aliases.get("deffov"), "exec hlsr-personalization.cfg");

  const result = migrate(root, "hl1-won", settings);
  assertSame(before, state(root, "hl1-won"));
  assert.deepEqual(launcherLeft(root, "hl1-won"), ["hlsr-personalization.cfg"]);
  assert.equal(fs.readFileSync(path.join(modDir(root, "hl1-won"), "hlsr-personalization.cfg"), "latin1"),
    goldsrcPersonalization.toCfgLines(settings));
  assert.equal(fs.readFileSync(path.join(result.backupDir, "Half-Life", "valve_WON", "hlsr-personalization.cfg"), "latin1"),
    goldsrcPersonalization.toCfgLines(old));
  // deffov во время игры: снимок выполняется после файла игрока — значения из settings.
  const runtime = runChain(reader(root, "hl1-won", EN), ["config.cfg", "hlsr-personalization.cfg"]);
  assert.equal(runtime.cvars.get("default_fov"), "110");
  assertSame(before, runtime);
});

test("без settings: шаблон на месте файла 0.4.8, hlsr-*.cfg на диске — их тексты в разделы, итог тот же", () => {
  const settings = hl1Settings();
  const root = cleanHl1(settings);
  const before = state(root, "hl1-won");
  assert.equal(playerConfig.ensure(root, "hl1-won", HL1, EN).created, true);
  const result = configMigration.migrate(root, "hl1-won", HL1, { ...EN, launcherVersion: VERSION, now: NOW });
  assert.equal(result.status, "migrated");
  assertSame(before, state(root, "hl1-won"));
  assert.deepEqual(launcherLeft(root, "hl1-won"), []);
  assert.equal(runAliases(textOf(root, "hl1-won")), 1);
});

test("Ghosting на Linux: нерабочий бинд сохранения исправлен, как 0.4.8 перед запуском; на Windows — нет", () => {
  for (const [platform, command] of [["linux", "save /; reload"], ["win32", "save :; reload"]]) {
    const settings = launchArgs.defaultSettings();
    const root = install("ghosting-renewed", {
      "config.cfg": 'bind "w" "+forward"\n',
      "autoexec.cfg": '  bind h "save :; reload"\r\ngh_opacity "175"\r\nexec hlsr-binds.cfg\r\nexec hlsr-personalization.cfg\r\n',
    }, settings);
    migrate(root, "ghosting-renewed", settings, { platform });
    assert.equal(state(root, "ghosting-renewed").binds.get("h"), command, platform);
    assert.ok(textOf(root, "ghosting-renewed").startsWith(`  bind h "${command}"\r\ngh_opacity "175"\r\n`), platform);
  }
});

test("I2: HL2 с 0.4.8 — переход идёт через путь приложения (prepareConfig, prepareForLaunch), опроса нет (проба 4)", () => {
  const defaultConfigs = require("../src/main/game-delivery/default-configs");
  const launchConfig = require("../src/main/game-delivery/manifests").launchConfigOf("hl2-2707");
  assert.equal(launchConfig.gameDirectory, GAME_DIRS["hl2-2707"]);
  const settings = launchArgs.defaultSettings();
  settings.binds = { f: { command: "+jump", base: "" } };
  const root = install("hl2-2707", { "config.cfg": 'bind "w" "+forward"\n', "autoexec.cfg": "sv_max_queries_sec 9\nexec hlsr-binds.cfg\n" }, settings);
  const before = state(root, "hl2-2707");
  assert.equal(before.binds.get("f"), "+jump");
  assert.deepEqual(playerConfig.setupState(root, "hl2-2707", "hl2", EN), { needed: false }, "установке 0.4.8 опрос не нужен");

  const result = defaultConfigs.prepareForLaunch("hl2-2707", root, launchConfig, settings, { ...EN, launcherVersion: VERSION, now: NOW });
  assert.equal(result.migrated, true);
  assertSame(before, state(root, "hl2-2707"));
  assert.deepEqual(launcherLeft(root, "hl2-2707"), []);
  assert.doesNotMatch(textOf(root, "hl2-2707"), /exec\s+hlsr-/i);
  assert.equal(JSON.parse(fs.readFileSync(path.join(root, ".hlsr", "config-setup.json"), "utf8")).migrated, VERSION);
  assert.deepEqual(playerConfig.setupState(root, "hl2-2707", "hl2", EN), { needed: false });

  // Чистая установка HL2: опрос нужен, подготовка кладёт шаблон, и опрос остаётся нужен.
  const clean = temp();
  assert.deepEqual(playerConfig.setupState(clean, "hl2-4260", "hl2", EN), { needed: true });
  assert.equal(defaultConfigs.prepareConfig("hl2-4260", clean, { gameDirectory: "hl2" }, EN).created, true);
  assert.equal(fs.readFileSync(playerConfig.locate(clean, "hl2-4260", "hl2", EN).file, "latin1"), playerConfig.template("hl2-4260"));
  assert.deepEqual(playerConfig.setupState(clean, "hl2-4260", "hl2", EN), { needed: true });
});

test("needsMigration и prepare: чистая установка — шаблон без перехода; с hlsr-*.cfg — переход", () => {
  const clean = temp();
  assert.equal(configMigration.needsMigration(clean, "hl1-won", HL1, EN), false);
  assert.deepEqual(playerConfig.prepare(clean, "hl1-won", HL1, { ...EN, settings: hl1Settings(), launcherVersion: VERSION }),
    { file: playerConfig.locate(clean, "hl1-won", HL1, EN).file, migrated: false, created: true });
  assert.equal(configMigration.needsMigration(clean, "hl1-won", HL1, EN), false);
  assert.equal(fs.existsSync(path.join(clean, ".hlsr")), false);

  const settings = hl1Settings();
  const root = cleanHl1(settings);
  assert.equal(configMigration.needsMigration(root, "hl1-won", HL1, EN), true);
  const prepared = playerConfig.prepare(root, "hl1-won", HL1, { ...EN, settings, launcherVersion: VERSION, now: NOW });
  assert.equal(prepared.migrated, true);
  assert.equal(prepared.created, false);
  assert.equal(configMigration.needsMigration(root, "hl1-won", HL1, EN), false);
});
