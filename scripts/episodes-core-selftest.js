"use strict";

// episodes.js: реестр эпизодов Ghosting, состояние, запуск, конфиг Ghosting в эпизод и обратно.
// node --test scripts/episodes-core-selftest.js

const assert = require("node:assert/strict");
const test = require("node:test");
const fs = require("fs");
const os = require("os");
const path = require("path");
const episodes = require("../src/main/game-delivery/episodes");

function library(t, files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-episodes-core-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const [relative, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(root, relative)), { recursive: true });
    fs.writeFileSync(path.join(root, relative), text);
  }
  const read = (relative) => (fs.existsSync(path.join(root, relative)) ? fs.readFileSync(path.join(root, relative), "utf8") : null);
  return { root, read };
}

const installedEp = (dir, version) => ({ [`${dir}/gameinfo.txt`]: "gameinfo", [`${dir}/${episodes.MARKER}`]: JSON.stringify({ version }) });

test("launchConfig: -game ghostingmod → -game ep2, исходная конфигурация не меняется", () => {
  const base = { gameDirectory: "ghostingmod", arguments: ["-steam", "-game", "ghostingmod"], steamAppId: 243730 };
  const ep2 = episodes.launchConfig(base, "ep2");
  assert.equal(ep2.gameDirectory, "ep2");
  assert.deepEqual(ep2.arguments, ["-steam", "-game", "ep2"]);
  assert.equal(ep2.steamAppId, 243730);
  assert.deepEqual(base.arguments, ["-steam", "-game", "ghostingmod"]);
  assert.equal(episodes.launchConfig(base, "ep1").gameDirectory, "episodic");
  assert.throws(() => episodes.launchConfig(base, "ep3"), { code: "LAUNCH_CONFIG_INVALID" });
  assert.throws(() => episodes.launchConfig({ arguments: ["-steam"] }, "ep1"), { code: "LAUNCH_CONFIG_INVALID" });
});

test("status: эпизод установлен, только если есть и маркер с версией, и gameinfo.txt", (t) => {
  const { root } = library(t, Object.assign({ "episodic/gameinfo.txt": "без маркера" }, installedEp("ep2", "1.0.0")));
  assert.deepEqual(episodes.status(root), { ep1: { installed: false, version: null }, ep2: { installed: true, version: "1.0.0" } });
  assert.deepEqual(episodes.status(null).ep1, { installed: false, version: null });
  const { root: noInfo } = library(t, { [`episodic/${episodes.MARKER}`]: JSON.stringify({ version: "1.0.0" }), [`ep2/gameinfo.txt`]: "g", [`ep2/${episodes.MARKER}`]: JSON.stringify({ version: 2 }) });
  assert.deepEqual(episodes.status(noInfo), { ep1: { installed: false, version: null }, ep2: { installed: false, version: null } }, "маркер без gameinfo, версия не строкой");
});

test("syncConfigIn: файлы игрока идут в эпизод, стоковые cfg Ghosting и эпизода не трогаются", (t) => {
  const { root, read } = library(t, Object.assign(installedEp("episodic", "1.0.0"), {
    "ghostingmod/cfg/autoexec.cfg": "мой autoexec",
    "ghostingmod/cfg/config.cfg": "bind 9 tc_n",
    "ghostingmod/cfg/tc_n.cfg": "мой скрипт",
    "ghostingmod/cfg/chapter9.cfg": "map d1_town_01",
    "ghostingmod/cfg/skill.cfg": "skill hl2",
    "ghostingmod/cfg/chapter1.cfg": "map d1_trainstation_01",
    "episodic/cfg/chapter1.cfg": "map ep1_citadel_00",
  }));
  const stock = new Set(["autoexec.cfg", "config.cfg", "CHAPTER9.cfg", "skill.cfg", "chapter1.cfg"]);
  const result = episodes.syncConfigIn(root, "ep1", { ghostingStock: stock });
  assert.deepEqual(result.copied.sort(), ["autoexec.cfg", "config.cfg", "tc_n.cfg"]);
  assert.equal(read("episodic/cfg/autoexec.cfg"), "мой autoexec");
  assert.equal(read("episodic/cfg/tc_n.cfg"), "мой скрипт");
  assert.equal(read("episodic/cfg/chapter9.cfg"), null, "главы HL2 не попадают в меню эпизода (регистр списка не важен)");
  assert.equal(read("episodic/cfg/skill.cfg"), null);
  assert.equal(read("episodic/cfg/chapter1.cfg"), "map ep1_citadel_00", "стоковый cfg эпизода не перезаписан");

  // Сборка Ghosting неизвестна — только autoexec.cfg и config.cfg; Config.cfg ложится как config.cfg.
  const { root: other, read: readOther } = library(t, Object.assign(installedEp("episodic", "1.0.0"), { "ghostingmod/cfg/Config.cfg": "x", "ghostingmod/cfg/tc_n.cfg": "y" }));
  assert.deepEqual(episodes.syncConfigIn(other, "ep1").copied, ["Config.cfg"]);
  assert.ok(fs.readdirSync(path.join(other, "episodic", "cfg")).includes("config.cfg"));
  assert.equal(readOther("episodic/cfg/config.cfg"), "x");

  // Нет ghostingmod\cfg или самого эпизода — без ошибок и без папок-призраков.
  const { root: bare } = library(t, installedEp("episodic", "1.0.0"));
  assert.deepEqual(episodes.syncConfigIn(bare, "ep1").copied, []);
  assert.deepEqual(episodes.syncConfigIn(bare, "ep2"), { copied: [], configSha256: null });
  assert.equal(fs.existsSync(path.join(bare, "ep2")), false);
});

test("syncConfigBack: config.cfg возвращается в Ghosting, только если игра его переписала", (t) => {
  const { root, read } = library(t, Object.assign(installedEp("ep2", "1.0.0"), { "ghostingmod/cfg/config.cfg": "bind 9 tc_n" }));
  const episodeConfig = path.join(root, "ep2", "cfg", "config.cfg");
  episodes.syncConfigIn(root, "ep2");
  assert.deepEqual(episodes.pendingSync(root), ["ep2"]);
  assert.deepEqual(episodes.syncConfigBack(root, "ep2"), { returned: false, conflict: false });
  assert.deepEqual(episodes.pendingSync(root), []);
  assert.deepEqual(episodes.pendingSync(null), []);

  episodes.syncConfigIn(root, "ep2");
  fs.writeFileSync(episodeConfig, "bind 0 tc_n");
  assert.deepEqual(episodes.syncConfigBack(root, "ep2"), { returned: true, conflict: false });
  assert.equal(read("ghostingmod/cfg/config.cfg"), "bind 0 tc_n");
  assert.deepEqual(episodes.syncConfigBack(root, "ep2"), { returned: false }, "повтор без отметки ничего не делает");
  assert.ok(!fs.readdirSync(path.join(root, "ghostingmod", "cfg")).some((n) => n.includes("tmp")), "временных файлов нет");

  // Пока эпизод висел (крах), config.cfg Ghosting поменяли — новее он, эпизодный не возвращается.
  episodes.syncConfigIn(root, "ep2");
  fs.writeFileSync(episodeConfig, "bind 1 old");
  fs.writeFileSync(path.join(root, "ghostingmod", "cfg", "config.cfg"), "bind 2 newer");
  assert.deepEqual(episodes.syncConfigBack(root, "ep2"), { returned: false, conflict: true });
  assert.equal(read("ghostingmod/cfg/config.cfg"), "bind 2 newer");

  // Игра упала на записи — пустой config.cfg не возвращается.
  episodes.syncConfigIn(root, "ep2");
  fs.writeFileSync(episodeConfig, "");
  assert.equal(episodes.syncConfigBack(root, "ep2").returned, false);
  assert.equal(read("ghostingmod/cfg/config.cfg"), "bind 2 newer");

  // Новый запуск поверх невозвращённого: сначала возврат, потом копия.
  episodes.syncConfigIn(root, "ep2");
  fs.writeFileSync(episodeConfig, "bind 3 from crash");
  episodes.syncConfigIn(root, "ep2");
  assert.equal(read("ghostingmod/cfg/config.cfg"), "bind 3 from crash");

  // Rename занят (Windows держит файл): временный файл убран, config.cfg Ghosting и отметка на месте.
  episodes.syncConfigIn(root, "ep2");
  fs.writeFileSync(episodeConfig, "bind 4 locked");
  const renameSync = fs.renameSync;
  t.mock.method(fs, "renameSync", () => { throw Object.assign(new Error("EBUSY"), { code: "EBUSY" }); });
  assert.throws(() => episodes.syncConfigBack(root, "ep2"), { code: "EBUSY" });
  fs.renameSync = renameSync;
  t.mock.restoreAll();
  assert.equal(read("ghostingmod/cfg/config.cfg"), "bind 3 from crash");
  assert.ok(!fs.readdirSync(path.join(root, "ghostingmod", "cfg")).some((n) => n.includes("tmp")), "временный файл убран");
  assert.deepEqual(episodes.pendingSync(root), ["ep2"], "отметка осталась — вернётся при следующем запуске");
  assert.equal(episodes.syncConfigBack(root, "ep2").returned, true);

  // Битая отметка снимается и не висит вечно.
  fs.writeFileSync(path.join(root, "ep2", episodes.SYNC), "{");
  assert.equal(episodes.syncConfigBack(root, "ep2").returned, false);
  assert.deepEqual(episodes.pendingSync(root), []);
});

test("remove: Episode One снимает и Episode Two — без него тот не запустится", (t) => {
  const { root, read } = library(t, Object.assign(installedEp("episodic", "1.0.0"), installedEp("ep2", "1.0.0"), { "ghostingmod/gameinfo.txt": "g" }));
  assert.deepEqual(episodes.dependentsOf("ep1"), ["ep2"]);
  assert.deepEqual(episodes.remove(root, "ep2").removed, ["ep2"]);
  assert.equal(read("episodic/gameinfo.txt"), "gameinfo", "Episode Two снимается один");
  fs.mkdirSync(path.join(root, "ep2", "cfg"), { recursive: true });
  assert.deepEqual(episodes.remove(root, "ep1").removed.sort(), ["ep2", "episodic"]);
  assert.equal(fs.existsSync(path.join(root, "episodic")), false);
  assert.equal(fs.existsSync(path.join(root, "ep2")), false);
  assert.equal(read("ghostingmod/gameinfo.txt"), "g");
  assert.throws(() => episodes.remove(root, "../x"), { code: "LAUNCH_CONFIG_INVALID" });
});
