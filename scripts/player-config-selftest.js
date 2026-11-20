"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const playerConfig = require("../src/main/game-delivery/player-config");
const cfgDocument = require("../src/shared/cfg-document");
const hl1Default = require("../src/main/game-delivery/hl1-default-userconfig");
const defaultConfigs = require("../src/main/game-delivery/default-configs");

const temp = () => fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-player-config-"));
const HL1 = "Half-Life/valve_WON";
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };
const RU = { language: "russian" };

test("locate: HL1 без языковой папки — valve_WON/userconfig.cfg", () => {
  const root = temp();
  const found = playerConfig.locate(root, "hl1-won", HL1, EN);
  assert.equal(found.file, path.join(root, "Half-Life", "valve_WON", "userconfig.cfg"));
  assert.equal(found.encoding, "latin1");
});

test("locate: языковая папка с config.cfg — файл игрока в ней", () => {
  const root = temp();
  const dir = path.join(root, "Half-Life", "valve_WON_russian");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, "config.cfg"), "");
  const found = playerConfig.locate(root, "hl1-won", HL1, RU);
  assert.equal(found.file, path.join(dir, "userconfig.cfg"));
});

test("locate: Ghosting и HL2 — autoexec.cfg в cfg", () => {
  const root = temp();
  assert.equal(playerConfig.locate(root, "ghosting-renewed", "ghostingmod", EN).file, path.join(root, "ghostingmod", "cfg", "autoexec.cfg"));
  assert.equal(playerConfig.locate(root, "hl2-4260", "hl2", EN).file, path.join(root, "hl2", "cfg", "autoexec.cfg"));
  assert.equal(playerConfig.locate(root, "unknown", "x", EN), null);
});

test("template: шесть заголовков по порядку, exec WON.cfg в rules, только ASCII", () => {
  const text = playerConfig.template("hl1-won");
  const at = Object.values(cfgDocument.SECTIONS).map((header) => text.indexOf(header));
  assert.ok(at.every((index) => index >= 0));
  assert.deepEqual([...at].sort((a, b) => a - b), at);
  assert.ok(text.indexOf("exec WON.cfg") > at[0] && text.indexOf("exec WON.cfg") < at[1]);
  assert.ok(text.indexOf("alias run") > at[1] && text.indexOf("alias run") < at[2]);
  assert.match(text, /^[\x00-\x7f]*$/);
  assert.ok(text.endsWith("\r\n") && !/[^\r]\n/.test(text));
});

test("template: HL2 и Ghosting", () => {
  assert.match(playerConfig.template("hl2-2707"), /sv_max_queries_sec 9\r\nsave_screenshot 0\r\ncl_showpos 1\r\ncl_showfps 1\r\n/);
  const ghosting = playerConfig.template("ghosting-renewed");
  assert.ok(ghosting.includes("gh_name \"myrun\"") && !/exec hlsr-/.test(ghosting));
});

test("ensure: пустая папка — создаёт шаблон", () => {
  const root = temp();
  const result = playerConfig.ensure(root, "hl1-won", HL1, EN);
  assert.equal(result.created, true);
  assert.equal(fs.readFileSync(result.file, "latin1"), playerConfig.template("hl1-won"));
});

test("ensure: файл игрока не трогает", () => {
  const root = temp();
  const file = playerConfig.locate(root, "hl1-won", HL1, EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, "default_fov 110\r\n");
  const result = playerConfig.ensure(root, "hl1-won", HL1, EN);
  assert.equal(result.created, false);
  assert.equal(fs.readFileSync(file, "latin1"), "default_fov 110\r\n");
});

test("ensure: стоковый userconfig 0.4.8 заменяется шаблоном", () => {
  const root = temp();
  const file = playerConfig.locate(root, "hl1-won", HL1, EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, hl1Default.TEXT + "exec hlsr-personalization.cfg\r\n");
  assert.equal(playerConfig.ensure(root, "hl1-won", HL1, EN).created, true);
  assert.equal(fs.readFileSync(file, "latin1"), playerConfig.template("hl1-won"));
});

test("ensure: файл появился между чтением и записью — не бросает, чужой файл цел", () => {
  const root = temp();
  const file = playerConfig.locate(root, "hl1-won", HL1, EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, "default_fov 110\r\n");
  let first = true;
  const lateFs = {
    ...fs,
    readFileSync: (...args) => {
      if (first) {
        first = false;
        throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      }
      return fs.readFileSync(...args);
    },
  };
  const result = playerConfig.ensure(root, "hl1-won", HL1, { ...EN, fs: lateFs });
  assert.equal(result.created, false);
  assert.equal(fs.readFileSync(file, "latin1"), "default_fov 110\r\n");
});

test("write: expectedSha256: undefined — без проверки", () => {
  const root = temp();
  const done = playerConfig.write(root, "hl1-won", HL1, "a\r\n", { ...EN, expectedSha256: undefined });
  assert.equal(fs.readFileSync(done.file, "latin1"), "a\r\n");
});

test("template HL1: подсказка игроку в разделе own, в settings только команды", () => {
  const text = playerConfig.template("hl1-won");
  const own = text.indexOf(cfgDocument.SECTIONS.own);
  assert.ok(text.indexOf("// Write your own") > own);
  assert.ok(text.indexOf("bxt_hud_incorrect_fps_indicator 0") < text.indexOf(cfgDocument.SECTIONS.personalization));
});

test("prepareConfig: HL1 без файла создаёт шаблон", () => {
  const root = temp();
  defaultConfigs.prepareConfig("hl1-won", root, { gameDirectory: HL1 }, { ...EN, settings: null, launcherVersion: "0.4.9" });
  assert.equal(fs.readFileSync(path.join(root, "Half-Life", "valve_WON", "userconfig.cfg"), "latin1"), playerConfig.template("hl1-won"));
});

test("write: верный expectedSha256 пишет, неверный — CONFIG_CHANGED без следов", () => {
  const root = temp();
  playerConfig.ensure(root, "hl1-won", HL1, EN);
  const before = playerConfig.read(root, "hl1-won", HL1, EN);
  assert.equal(before.exists, true);
  const done = playerConfig.write(root, "hl1-won", HL1, before.text + "default_fov 110\r\n", { ...EN, expectedSha256: before.sha256 });
  assert.equal(playerConfig.read(root, "hl1-won", HL1, EN).sha256, done.sha256);

  const bytes = fs.readFileSync(done.file);
  assert.throws(
    () => playerConfig.write(root, "hl1-won", HL1, "x\r\n", { ...EN, expectedSha256: before.sha256 }),
    (error) => error instanceof playerConfig.PlayerConfigError && error.code === "CONFIG_CHANGED"
  );
  assert.deepEqual(fs.readFileSync(done.file), bytes);
  assert.deepEqual(fs.readdirSync(path.dirname(done.file)), ["userconfig.cfg"]);
});

test("write: файл появился, пока ждали null — CONFIG_CHANGED", () => {
  const root = temp();
  const file = playerConfig.locate(root, "hl1-won", HL1, EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, "a\r\n");
  assert.throws(
    () => playerConfig.write(root, "hl1-won", HL1, "b\r\n", { ...EN, expectedSha256: null }),
    (error) => error.code === "CONFIG_CHANGED"
  );
  assert.equal(fs.readFileSync(file, "latin1"), "a\r\n");
});

test("latin1: байты 0x80-0xFF вне правленой строки не меняются", () => {
  const root = temp();
  const file = playerConfig.locate(root, "hl1-won", HL1, EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const head = Buffer.from([0x2f, 0x2f, 0x20, 0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2, 0x80, 0xff, 0x0d, 0x0a]);
  const tail = Buffer.from("sensitivity 3\r\n", "latin1");
  fs.writeFileSync(file, Buffer.concat([head, tail]));
  const read = playerConfig.read(root, "hl1-won", HL1, EN);
  const doc = cfgDocument.set(cfgDocument.parse(read.text), "cvar:sensitivity", "4", "settings");
  playerConfig.write(root, "hl1-won", HL1, cfgDocument.serialize(doc), { ...EN, expectedSha256: read.sha256 });
  const after = fs.readFileSync(file);
  assert.deepEqual(after.subarray(0, head.length), head);
  assert.ok(after.includes(Buffer.from("sensitivity 4", "latin1")));
});

test("isStock: пусто и шаблон — да, файл игрока — нет", () => {
  assert.equal(playerConfig.isStock("hl1-won", ""), true);
  assert.equal(playerConfig.isStock("hl1-won", playerConfig.template("hl1-won")), true);
  assert.equal(playerConfig.isStock("hl1-won", "default_fov 110\r\n"), false);
  assert.equal(playerConfig.isStock("hl1-won", hl1Default.STOCK + "\r\nexec hlsr-binds.cfg\r\n"), true);
});

// C3 (ruling R17): autoexec Source из Блокнота в ANSI (cp1251) переживает любую запись байт в байт.
const CP1251 = fs.readFileSync(path.join(__dirname, "fixtures", "single-config", "cp1251-autoexec.cfg"));
const GHOST = "ghostingmod";

function ghosting(bytes) {
  const root = temp();
  const file = playerConfig.locate(root, "ghosting-renewed", GHOST, EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes);
  return { root, file };
}

test("C3: фикстура cp1251 — не UTF-8, кириллица в комментариях", () => {
  assert.equal(CP1251.length, 187);
  assert.equal(require("crypto").createHash("sha256").update(CP1251).digest("hex"), "6db4389d2029b761aceed2d0ba75e0dd7395ab7865197d68344f65c5b4f464a2");
  assert.throws(() => new TextDecoder("utf-8", { fatal: true }).decode(CP1251));
});

test("C3: cp1251 в autoexec Source — персонализация и бинды не трогают чужие байты (проба 1C)", () => {
  const launchArgs = require("../src/main/game-delivery/launch-args");
  const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
  const a = ghosting(CP1251);
  const stored = launchArgs.defaultSettings();
  const shown = playerConfig.readPersonalization(a.root, "ghosting-renewed", GHOST, stored, EN);
  assert.deepEqual(shown.personalization.crossColor, { r: 255, g: 0, b: 0 }, "значения из файла читаются");
  assert.equal(shown.personalization.extra.gh_name, "myrun");
  const next = { ...shown, personalization: { ...shown.personalization, crossSize: 14 } };
  playerConfig.writePersonalization(a.root, "ghosting-renewed", GHOST, stored, next, EN);
  const after = fs.readFileSync(a.file);
  assert.deepEqual(after.subarray(0, CP1251.length), CP1251);
  assert.match(after.toString("latin1"), /gh_cross_size "14"/);

  const b = ghosting(CP1251);
  gameBindsInstall.applyBinds(b.root, "ghosting-renewed", GHOST, { f: { command: "+use", base: "+jump" }, g: { command: "+duck", base: "" } }, EN);
  const binds = fs.readFileSync(b.file);
  const edited = Buffer.from(CP1251.toString("latin1").replace('"+jump"', '"+use"'), "latin1");
  assert.deepEqual(binds.subarray(0, edited.length), edited, "правка на месте — только значение, комментарий cp1251 цел");
  assert.match(binds.toString("latin1"), /bind "g" "\+duck"/);
});

test("C3: шаблон Ghosting с кириллицей пишется байтами UTF-8; прочитанный, он остаётся стоковым", () => {
  const root = temp();
  const result = playerConfig.ensure(root, "ghosting-renewed", GHOST, EN);
  assert.equal(result.created, true);
  assert.deepEqual(fs.readFileSync(result.file), Buffer.from(playerConfig.template("ghosting-renewed"), "utf8"));
  const read = playerConfig.read(root, "ghosting-renewed", GHOST, EN);
  assert.equal(playerConfig.isStock("ghosting-renewed", read.text), true);
  assert.equal(playerConfig.ensure(root, "ghosting-renewed", GHOST, EN).created, true, "шаблон заменяется шаблоном, байты те же");
  assert.deepEqual(fs.readFileSync(result.file), Buffer.from(playerConfig.template("ghosting-renewed"), "utf8"));
});

test("C3: импорт cp1251 через окно выбора (latin1 + cfgText) — файл игрока байт в байт", () => {
  const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
  const cfgImport = require("../src/shared/cfg-import");
  const gameBinds = require("../src/shared/game-binds");
  const c = ghosting(Buffer.from("// old\r\n"));
  const built = cfgImport.flattenImport({ engine: "source", root: { name: "autoexec.cfg", text: gameBinds.cfgText(CP1251.toString("latin1")) }, files: {}, missing: [] });
  assert.equal(gameBindsInstall.importPlayerConfig(c.root, "ghosting-renewed", GHOST, built, EN).status, "ok");
  assert.deepEqual(fs.readFileSync(c.file), CP1251);
});

test("M1: у HL2 есть отпечаток файла игрока — «Клавиатура» сохраняет со сверкой", () => {
  const root = temp();
  const stored = { binds: {} };
  assert.deepEqual(playerConfig.inspectPersonalization(root, "hl2-4260", "hl2", stored, EN), { settings: stored, sha256: null });
  const file = playerConfig.locate(root, "hl2-4260", "hl2", EN).file;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, 'bind "f" "+jump"\r\n');
  const inspected = playerConfig.inspectPersonalization(root, "hl2-4260", "hl2", stored, EN);
  assert.equal(inspected.sha256, require("crypto").createHash("sha256").update(fs.readFileSync(file)).digest("hex"));
  assert.equal(inspected.settings, stored);
  assert.equal(playerConfig.inspectPersonalization(root, "unknown", "x", stored, EN).sha256, undefined);
});

test("M2: файл игрока и config.cfg в языковой папке HL1 — данные игрока при переустановке", () => {
  const mutablePaths = require("../src/main/game-delivery/mutable-paths");
  const userData = mutablePaths.createMatcher(mutablePaths.USER_DATA);
  for (const file of ["Half-Life/valve_WON_russian/userconfig.cfg", "Half-Life/valve_WON_russian/config.cfg", "Half-Life/valve_WON/userconfig.cfg"])
    assert.equal(userData(file), true, file);
  assert.equal(userData("Half-Life/valve_WON_russian/WON.cfg"), false, "файлы сборки не в счёт");
});

const MOD = path.join("Half-Life", "valve_WON");
const SETUP = (root) => path.join(root, ".hlsr", "config-setup.json");

test("setupState: чистая установка — спрашиваем; шаблон или стоковый файл не мешают", () => {
  const root = temp();
  assert.deepEqual(playerConfig.setupState(root, "hl1-won", HL1, EN), { needed: true });
  playerConfig.ensure(root, "hl1-won", HL1, EN);
  assert.deepEqual(playerConfig.setupState(root, "hl1-won", HL1, EN), { needed: true });
});

test("setupState: свой userconfig.cfg — не спрашиваем", () => {
  const root = temp();
  fs.mkdirSync(path.join(root, MOD), { recursive: true });
  fs.writeFileSync(path.join(root, MOD, "userconfig.cfg"), "default_fov 110\r\n");
  assert.deepEqual(playerConfig.setupState(root, "hl1-won", HL1, EN), { needed: false });
});

test("setupState: установка 0.4.8, которой нужен переход, — не спрашиваем", () => {
  const root = temp();
  fs.mkdirSync(path.join(root, MOD), { recursive: true });
  fs.writeFileSync(path.join(root, MOD, "userconfig.cfg"), "exec hlsr-binds.cfg\r\n");
  assert.deepEqual(playerConfig.setupState(root, "hl1-won", HL1, EN), { needed: false });
  assert.equal(fs.existsSync(SETUP(root)), false, "состояние ничего не пишет");
});

test("setupDone: scratch и skip создают шаблон и отметку, повторный вопрос не нужен", () => {
  for (const choice of ["scratch", "skip"]) {
    const root = temp();
    playerConfig.setupDone(root, "hl1-won", HL1, choice, EN);
    assert.equal(fs.readFileSync(path.join(root, MOD, "userconfig.cfg"), "latin1"), playerConfig.template("hl1-won"));
    const mark = JSON.parse(fs.readFileSync(SETUP(root), "utf8"));
    assert.equal(mark.choice, choice);
    assert.ok(!Number.isNaN(Date.parse(mark.at)));
    assert.deepEqual(playerConfig.setupState(root, "hl1-won", HL1, EN), { needed: false });
  }
});

test("setupDone: own файл не создаёт; чужие поля отметки сохраняются; мусор отклоняется", () => {
  const root = temp();
  fs.mkdirSync(path.join(root, ".hlsr"), { recursive: true });
  fs.writeFileSync(SETUP(root), JSON.stringify({ migrated: "0.4.9" }));
  playerConfig.setupDone(root, "hl1-won", HL1, "own", EN);
  assert.equal(fs.existsSync(path.join(root, MOD, "userconfig.cfg")), false);
  const mark = JSON.parse(fs.readFileSync(SETUP(root), "utf8"));
  assert.equal(mark.migrated, "0.4.9");
  assert.equal(mark.choice, "own");
  assert.deepEqual(fs.readdirSync(path.join(root, ".hlsr")), ["config-setup.json"], "временных файлов нет");
  assert.throws(() => playerConfig.setupDone(root, "hl1-won", HL1, "other", EN), { code: "CONFIG_SETUP_CHOICE" });
});
