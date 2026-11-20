"use strict";

// Импорт одним файлом (§8.1) и экспорт (§8.2): цепочка exec сливается в файл игрока
// с тем же итогом в игре (эмулятор cfg), скрипты остаются файлами, прежнее
// возвращается байт в байт.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const { runChain } = require("./lib/cfg-state");
const cfgImport = require("../src/shared/cfg-import");
const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const playerConfig = require("../src/main/game-delivery/player-config");

const GAME_DIR = "Half-Life/valve_WON";
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
// Какую копию записал лаунчер — в папке лаунчера (backupsFile), не в папке игры.
const LAUNCHER_DIR = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-launcher-"));
test.after(() => fs.rmSync(LAUNCHER_DIR, { recursive: true, force: true }));
const OPTIONS = { language: "english", now: new Date(2026, 10, 6, 12, 0, 0).getTime(),
  backupsFile: path.join(LAUNCHER_DIR, "player-config-backups.json") };
const flatten = (root, files = {}, missing = []) => cfgImport.flattenImport({ engine: "goldsrc", root, files, missing });
const fileOf = (name, text, target) => ({ name, target: target || name, text });

function chainState(texts, root) {
  return runChain((name) => (name in texts ? texts[name] : null), [root]);
}

function install(t, files = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-flatten-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const modDir = path.join(root, GAME_DIR);
  fs.mkdirSync(modDir, { recursive: true });
  for (const [name, bytes] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(modDir, name)), { recursive: true });
    fs.writeFileSync(path.join(modDir, name), bytes);
  }
  return { root, modDir };
}

test("цепочка из трёх файлов сливается в порядке выполнения, итог в игре тот же", () => {
  const root = { name: "main.cfg", text: "fps_max 100\nexec a.cfg\nbind f +use\nsensitivity 3\n" };
  const a = "bind a +jump\nexec \"b.cfg\"\nfps_max 250\n";
  const b = "sensitivity 2\nbind f +attack\n";
  const built = flatten(root, { "a.cfg": fileOf("a.cfg", a), "b.cfg": fileOf("b.cfg", b) });
  assert.deepEqual(built.runtimeFiles, []);
  assert.match(built.text, /^\/\/ ===== from a\.cfg =====$/m);
  assert.match(built.text, /^\/\/ ===== from b\.cfg =====$/m);
  assert.ok(built.text.indexOf("from a.cfg") < built.text.indexOf("from b.cfg"));
  const before = chainState({ "main.cfg": root.text, "a.cfg": a, "b.cfg": b }, "main.cfg");
  const after = chainState({ "main.cfg": built.text }, "main.cfg");
  assert.deepEqual(after.cvars, before.cvars);
  assert.deepEqual(after.binds, before.binds);
  assert.deepEqual(after.aliases, before.aliases);
  assert.equal(after.cvars.get("sensitivity"), "3");
  assert.equal(after.binds.get("f"), "+use");
});

test("exec внутри alias и bind не разворачивается, файл остаётся файлом", () => {
  const root = { name: "main.cfg", text: 'alias go "exec scripts/run.cfg"\nbind g "exec scripts/run.cfg; say hi"\nbind h +use\n' };
  const run = "bind k +jump\nexec deep.cfg\n";
  const built = flatten(root, { "scripts/run.cfg": fileOf("run.cfg", run, "scripts/run.cfg"), "deep.cfg": fileOf("deep.cfg", "fps_max 20\n") });
  assert.doesNotMatch(built.text, /from scripts\/run\.cfg/);
  assert.match(built.text, /alias go "exec scripts\/run\.cfg"/);
  assert.deepEqual(built.runtimeFiles.map((item) => item.file), ["scripts/run.cfg"]);
  assert.match(built.runtimeFiles[0].text, /from deep\.cfg/, "exec верхнего уровня в самом скрипте тоже сливается");
  assert.ok(cfgImport.isSafeImportName(built.runtimeFiles[0].file));
});

test("цикл a→b→a оборван комментарием, exec игровых файлов закомментирован", () => {
  const built = flatten({ name: "a.cfg", text: "bind a +use\nexec b.cfg\nexec userconfig.cfg\nexec gone.cfg\n" },
    { "b.cfg": fileOf("b.cfg", "bind b +jump\nexec a.cfg\n") }, ["gone.cfg"]);
  assert.match(built.text, /^\/\/ HLSR: exec a\.cfg skipped \(cycle\)$/m);
  assert.match(built.text, /^\/\/ HLSR: exec userconfig\.cfg skipped \(not imported\)$/m);
  assert.match(built.text, /^\/\/ HLSR: exec gone\.cfg skipped \(not imported\)$/m);
  assert.deepEqual(built.report.cycles, [["b.cfg", "a.cfg"]]);
  assert.equal(chainState({ "a.cfg": built.text }, "a.cfg").binds.size, 2);
});

test("импорт, затем undo возвращают байты прежнего файла и скриптов", (t) => {
  const own = Buffer.from([0x62, 0x69, 0x6e, 0x64, 0x20, 0x71, 0x20, 0x2b, 0x75, 0x73, 0x65, 0x20, 0x2f, 0x2f, 0xe0, 0xe1, 0x0d, 0x0a]);
  const oldRun = Buffer.from("// прежний скрипт\r\n", "utf8");
  const { root, modDir } = install(t, { "userconfig.cfg": own, "scripts/run.cfg": oldRun });
  const built = flatten({ name: "main.cfg", text: 'bind z "exec scripts/Run.cfg"\nexec other.cfg\n' },
    { "scripts/run.cfg": fileOf("Run.cfg", "bind k +jump\n", "scripts/Run.cfg"), "other.cfg": fileOf("other.cfg", "fps_max 50\n") });
  const sha = playerConfig.read(root, "hl1-won", GAME_DIR, OPTIONS).sha256;

  assert.equal(gameBindsInstall.importPlayerConfig(root, "hl1-won", GAME_DIR, built, { ...OPTIONS, expectedSha256: "stale" }).status, "changed");
  assert.deepEqual(fs.readFileSync(path.join(modDir, "userconfig.cfg")), own, "чужой отпечаток — файл не тронут");

  const result = gameBindsInstall.importPlayerConfig(root, "hl1-won", GAME_DIR, built, { ...OPTIONS, expectedSha256: sha });
  assert.equal(result.status, "ok");
  assert.match(result.backup.replace(/\\/g, "/"), /^\.hlsr\/backup\/20261106-120000$/);
  const written = fs.readFileSync(path.join(modDir, "userconfig.cfg"), "latin1");
  assert.match(written, /fps_max 50/);
  assert.equal(result.sha256, playerConfig.read(root, "hl1-won", GAME_DIR, OPTIONS).sha256);
  assert.equal(fs.readFileSync(path.join(modDir, "scripts/run.cfg"), "latin1"), "bind k +jump\r\n");
  assert.deepEqual(fs.readFileSync(path.join(root, result.backup, GAME_DIR, "userconfig.cfg")), own, "копия — прежние байты");

  // Второй импорт в ту же секунду не затирает первую копию.
  const second = gameBindsInstall.importPlayerConfig(root, "hl1-won", GAME_DIR,
    flatten({ name: "m.cfg", text: "bind y +duck\n" }), OPTIONS);
  assert.notEqual(second.backup, result.backup);

  assert.equal(gameBindsInstall.undoImport(root, "hl1-won", GAME_DIR, OPTIONS).status, "ok");
  assert.match(fs.readFileSync(path.join(modDir, "userconfig.cfg"), "latin1"), /fps_max 50/, "первый откат — состояние после первого импорта");
  // R18: откат сначала кладёт текущие файлы в копию — следующий «Вернуть прежний» отменяет сам откат.
  assert.equal(gameBindsInstall.undoImport(root, "hl1-won", GAME_DIR, OPTIONS).status, "ok");
  assert.match(fs.readFileSync(path.join(modDir, "userconfig.cfg"), "latin1"), /bind y \+duck/, "возврат отката — второй импорт");

  // Один импорт и откат: прежние байты файла игрока и скрипта.
  const single = install(t, { "userconfig.cfg": own, "scripts/run.cfg": oldRun });
  gameBindsInstall.importPlayerConfig(single.root, "hl1-won", GAME_DIR, built, OPTIONS);
  const last = gameBindsInstall.undoImport(single.root, "hl1-won", GAME_DIR, OPTIONS);
  assert.equal(last.status, "ok");
  assert.deepEqual(fs.readFileSync(path.join(single.modDir, "userconfig.cfg")), own);
  assert.deepEqual(fs.readFileSync(path.join(single.modDir, "scripts/run.cfg")), oldRun);
  assert.equal(last.sha256, sha);
});

test("undo удаляет файлы, которых до импорта не было", (t) => {
  const { root, modDir } = install(t, { "userconfig.cfg": "bind q +use\r\n" });
  const built = flatten({ name: "m.cfg", text: 'alias go "exec fresh.cfg"\n' }, { "fresh.cfg": fileOf("fresh.cfg", "bind k +jump\n") });
  gameBindsInstall.importPlayerConfig(root, "hl1-won", GAME_DIR, built, OPTIONS);
  assert.ok(fs.existsSync(path.join(modDir, "fresh.cfg")));
  gameBindsInstall.undoImport(root, "hl1-won", GAME_DIR, OPTIONS);
  assert.equal(fs.existsSync(path.join(modDir, "fresh.cfg")), false);
});

test("undo берёт только копию, которую записал лаунчер: подложенная .hlsrpack копия и правленый список не действуют", (t) => {
  const own = "bind q +use\r\n";
  const { root, modDir } = install(t, { "userconfig.cfg": own, "liblist.gam": "game \"Half-Life\"\r\n", "my.dem": "demo" });
  const planted = path.join(root, ".hlsr", "backup", "99999999-999999");
  fs.mkdirSync(path.join(planted, GAME_DIR), { recursive: true });
  fs.writeFileSync(path.join(planted, GAME_DIR, "liblist.gam"), "чужой");
  fs.writeFileSync(path.join(planted, "import.json"), JSON.stringify({
    kept: [path.join(GAME_DIR, "liblist.gam")], created: [path.join(GAME_DIR, "my.dem")],
  }));
  const fresh = (backupsFile) => gameBindsInstall.undoImport(root, "hl1-won", GAME_DIR, { ...OPTIONS, backupsFile });
  assert.equal(fresh(path.join(root, "нет.json")).status, "nothing", "без записи лаунчера копий нет");

  const result = gameBindsInstall.importPlayerConfig(root, "hl1-won", GAME_DIR, flatten({ name: "m.cfg", text: "bind y +duck\n" }), OPTIONS);
  // Список настоящей копии правлен: SHA-256 не совпал — возврата нет, файлы на месте.
  const manifest = path.join(root, result.backup, "import.json");
  const original = fs.readFileSync(manifest);
  fs.writeFileSync(manifest, JSON.stringify({ kept: [], created: [path.join(GAME_DIR, "my.dem")] }));
  assert.equal(gameBindsInstall.undoImport(root, "hl1-won", GAME_DIR, OPTIONS).status, "nothing");
  assert.ok(fs.existsSync(path.join(modDir, "my.dem")));
  fs.writeFileSync(manifest, original);

  assert.equal(gameBindsInstall.undoImport(root, "hl1-won", GAME_DIR, OPTIONS).status, "ok");
  assert.equal(fs.readFileSync(path.join(modDir, "userconfig.cfg"), "latin1"), own);
  assert.equal(fs.readFileSync(path.join(modDir, "liblist.gam"), "latin1"), "game \"Half-Life\"\r\n");
  assert.ok(fs.existsSync(path.join(modDir, "my.dem")));
  assert.ok(fs.existsSync(path.join(planted, "import.json")), "подложенная копия не тронута");

  // Даже со своим отпечатком: «..», абсолютные пути и «:» в списке отвергаются.
  for (const bad of [path.join("..", "outside.txt"), path.join(root, "x.cfg"), "C:x.cfg"]) {
    const text = JSON.stringify({ kept: [], created: [bad] });
    fs.writeFileSync(path.join(planted, "import.json"), text);
    const backupsFile = path.join(LAUNCHER_DIR, "bad.json");
    fs.writeFileSync(backupsFile, JSON.stringify({ "hl1-won": {
      stamp: "99999999-999999", sha256: require("crypto").createHash("sha256").update(text).digest("hex"),
    } }));
    assert.throws(() => fresh(backupsFile), (error) => error.code === "PATH_INVALID", bad);
  }
});

test("экспорт без файлов скриптов пишет один .cfg", async (t) => {
  const { root } = install(t, { "userconfig.cfg": "bind q +use\r\n" });
  const out = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-export-"));
  t.after(() => fs.rmSync(out, { recursive: true, force: true }));
  let asked = null;
  const result = await gameBindsInstall.exportPlayerConfig(root, "hl1-won", GAME_DIR, {
    ...OPTIONS,
    chooseFile: async (name) => { asked = name; return path.join(out, name); },
    chooseDirectory: async () => { throw new Error("папка не нужна"); },
  });
  assert.equal(asked, "hl1-won-userconfig.cfg");
  assert.equal(result.status, "ok");
  assert.equal(fs.readFileSync(path.join(out, asked), "latin1"), "bind q +use\r\n");
  const canceled = await gameBindsInstall.exportPlayerConfig(root, "hl1-won", GAME_DIR, { ...OPTIONS, chooseFile: async () => null });
  assert.equal(canceled.status, "canceled");
});

test("экспорт с файлами, которые зовёт alias, пишет папку с подпапками", async (t) => {
  const { root } = install(t, {
    "userconfig.cfg": 'alias go "exec scripts/run.cfg"\r\n',
    "scripts/run.cfg": 'alias more "exec tail.cfg"\r\n',
    "tail.cfg": "fps_max 20\r\n",
    "unused.cfg": "x\r\n",
  });
  const out = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-export-"));
  t.after(() => fs.rmSync(out, { recursive: true, force: true }));
  const result = await gameBindsInstall.exportPlayerConfig(root, "hl1-won", GAME_DIR, {
    ...OPTIONS, chooseFile: async () => { throw new Error("нужна папка"); }, chooseDirectory: async () => out,
  });
  assert.equal(result.status, "ok");
  assert.equal(result.path, path.join(out, "hl1-won-config-20261106-120000"), "ответ — путь новой подпапки");
  assert.deepEqual(fs.readdirSync(result.path).sort(), ["scripts", "tail.cfg", "userconfig.cfg"]);
  assert.equal(fs.readFileSync(path.join(result.path, "scripts/run.cfg"), "latin1"), 'alias more "exec tail.cfg"\r\n');
});

test("экспорт в папку не трогает существующие файлы, занятую подпапку не берёт", async (t) => {
  const { root } = install(t, { "userconfig.cfg": 'alias go "exec scripts/run.cfg"\r\n', "scripts/run.cfg": "fps_max 20\r\n" });
  const out = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-export-"));
  t.after(() => fs.rmSync(out, { recursive: true, force: true }));
  fs.mkdirSync(path.join(out, "scripts"));
  fs.writeFileSync(path.join(out, "userconfig.cfg"), "чужой");
  fs.writeFileSync(path.join(out, "scripts/run.cfg"), "чужой скрипт");
  const options = { ...OPTIONS, chooseFile: async () => null, chooseDirectory: async () => out };
  const first = await gameBindsInstall.exportPlayerConfig(root, "hl1-won", GAME_DIR, options);
  const second = await gameBindsInstall.exportPlayerConfig(root, "hl1-won", GAME_DIR, options);
  assert.equal(fs.readFileSync(path.join(out, "userconfig.cfg"), "utf8"), "чужой");
  assert.equal(fs.readFileSync(path.join(out, "scripts/run.cfg"), "utf8"), "чужой скрипт");
  assert.notEqual(first.path, second.path);
  assert.equal(fs.readFileSync(path.join(first.path, "userconfig.cfg"), "latin1"), 'alias go "exec scripts/run.cfg"\r\n');
  assert.ok(fs.existsSync(path.join(second.path, "scripts/run.cfg")));
});

test("в строках лаунчера имя файла только ASCII и без «;», итог в игре не меняется", () => {
  const root = { name: "main.cfg", text: 'bind a +use\nexec "файл.cfg"\nexec "x;bind b +jump.cfg"\nexec userconfig.cfg\n' };
  const files = {
    "файл.cfg": fileOf("файл.cfg", "bind c +duck\n"),
    "x;bind b +jump.cfg": fileOf("x;bind b +jump.cfg", "bind d +attack\n"),
  };
  const built = flatten(root, files);
  const headers = built.text.split("\n").filter((line) => /^\/\/ (=====|HLSR)/.test(line));
  assert.equal(headers.length, 3);
  for (const line of headers) assert.match(line, /^[\x20-\x7e]*$/);
  assert.ok(headers.every((line) => !line.includes(";")));
  const after = chainState({ "main.cfg": built.text }, "main.cfg");
  assert.deepEqual([...after.binds.keys()].sort(), ["a", "c", "d"], "bind b из имени файла не появился");
});

test("Source: alias из файла и бинд на него попадают в файл игрока", (t) => {
  const { root, modDir } = install(t, {});
  const cfgDir = path.join(root, "hl2/cfg");
  fs.mkdirSync(cfgDir, { recursive: true });
  const built = cfgImport.flattenImport({ engine: "source",
    root: { name: "autoexec.cfg", text: 'alias hop "+jump; wait 2; -jump"\nbind f hop\nexec extra\n' },
    files: { "extra.cfg": fileOf("extra.cfg", "alias spin \"+left\"\nbind g spin\n") } });
  const result = gameBindsInstall.importPlayerConfig(root, "hl2-2707", "hl2", built, OPTIONS);
  assert.equal(result.status, "ok");
  const written = fs.readFileSync(result.file, "utf8");
  assert.match(written, /alias hop "\+jump; wait 2; -jump"/);
  assert.match(written, /^bind f hop$/m);
  assert.match(written, /^alias spin "\+left"$/m);
  assert.match(written, /^bind g spin$/m);
  assert.equal(built.summary.aliases, 2);
  assert.equal(modDir.length > 0, true);
});

test("окно импорта: полный импорт идёт для любого движка, не только GoldSrc", () => {
  const source = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameSettingsModal.vue"), "utf8");
  assert.match(source, /wholeCfgImport\(\) \{\s*return !!this\.bindsGameId && !!bindPresets\.engineFor\(this\.bindsGameId\);/);
  assert.match(source, /"player-config-import-apply"/);
  assert.match(source, /engine === "goldsrc" \? this\.adoptImport/);
});
