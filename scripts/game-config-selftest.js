"use strict";

// Редактор конфигов: разбор цепочки cfg, запись на место и выбор файла
// (src/shared/game-config.js, src/main/game-delivery/game-config-install.js).

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const gameConfig = require("../src/shared/game-config");
const install = require("../src/main/game-delivery/game-config-install");
const goldsrcSettings = require("../src/shared/goldsrc-settings");
const ghostingSettings = require("../src/shared/ghosting-settings");
const ghostingPersonalization = require("../src/main/game-delivery/ghosting-personalization");
const goldsrcPersonalization = require("../src/main/game-delivery/goldsrc-personalization");

const SOURCE = { gameDirectory: "hl2" };
const GHOSTING = { gameDirectory: "ghostingmod" };
const GOLDSRC = { gameDirectory: "Half-Life/valve_WON" };

const roots = [];
test.after(() => roots.forEach((root) => fs.rmSync(root, { recursive: true, force: true })));

function tempInstall(files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-cfg-"));
  roots.push(root);
  for (const [name, text] of Object.entries(files)) {
    const file = path.join(root, name);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, Buffer.from(text, name.startsWith("Half-Life") ? "latin1" : "utf8"));
  }
  return root;
}

const read = (root, name, encoding = "utf8") => fs.readFileSync(path.join(root, name), encoding);

test("значение берётся из файла, где движок задаёт его последним", () => {
  const root = tempInstall({
    "hl2/cfg/config.cfg": 'sensitivity "2"\nvolume "0.5"\n',
    "hl2/cfg/autoexec.cfg": "exec mine\n",
    "hl2/cfg/mine.cfg": "sensitivity 4.5 // моя сенса\n",
  });
  const result = install.readConfig(root, SOURCE, "hl2-4260", "source", { language: null });
  assert.equal(result.values.sensitivity, "4.5");
  assert.equal(result.sources.sensitivity, "mine.cfg");
  assert.equal(result.values.volume, "0.5");
});

test("правка остаётся на своей строке: отступ, кавычки и комментарий не меняются", () => {
  const root = tempInstall({
    "hl2/cfg/config.cfg": 'unbindall\r\nbind "w" "+forward"\r\nsensitivity "2"\r\n',
    "hl2/cfg/autoexec.cfg": "  sensitivity 4.5 // моя сенса\nfps_max 300; volume 0.4\n",
  });
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { sensitivity: 2.25, volume: 70 }, { language: null });
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"), "  sensitivity 2.25 // моя сенса\nfps_max 300; volume 0.7\n");
  assert.equal(read(root, "hl2/cfg/config.cfg"), 'unbindall\r\nbind "w" "+forward"\r\nsensitivity "2"\r\n');
});

test("переменные, которых нигде нет, уходят в раздел settings файла игрока; config.cfg не тронут", () => {
  const root = tempInstall({ "hl2/cfg/config.cfg": 'sensitivity "2"\r\n', "hl2/cfg/autoexec.cfg": "" });
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { m_filter: true, cl_showfps: 2, m_pitch: true }, { language: null });
  assert.equal(read(root, "hl2/cfg/config.cfg"), 'sensitivity "2"\r\n');
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"),
    '// ===== HLSR: settings =====\r\nm_filter "1"\r\ncl_showfps "2"\r\nm_pitch "-0.022"\r\n');
  assert.equal(fs.existsSync(path.join(root, "hl2/cfg/hlsr-config.cfg")), false, "hlsr-config.cfg не создаётся");

  // Второй раз значение правится на месте, раздел один.
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { cl_showfps: 0 }, { language: null });
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"),
    '// ===== HLSR: settings =====\r\nm_filter "1"\r\ncl_showfps "0"\r\nm_pitch "-0.022"\r\n');
});

test("переменная только в config.cfg пишется в файл игрока, config.cfg остаётся прежним", () => {
  const root = tempInstall({ "hl2/cfg/config.cfg": 'sensitivity "2"\n', "hl2/cfg/autoexec.cfg": "cl_showpos 1\n" });
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { sensitivity: 3 }, { language: null });
  assert.equal(read(root, "hl2/cfg/config.cfg"), 'sensitivity "2"\n');
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"), 'cl_showpos 1\n\n// ===== HLSR: settings =====\nsensitivity "3"\n');
  assert.equal(install.readConfig(root, SOURCE, "hl2-4260", "source", { language: null }).values.sensitivity, "3");
});

test("инверсия сохраняет величину m_pitch", () => {
  const root = tempInstall({ "hl2/cfg/config.cfg": 'm_pitch "0.03"\n', "hl2/cfg/autoexec.cfg": "" });
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { m_pitch: true }, { language: null });
  assert.equal(read(root, "hl2/cfg/config.cfg"), 'm_pitch "0.03"\n');
  assert.match(read(root, "hl2/cfg/autoexec.cfg"), /m_pitch "-0\.03"/);
  assert.equal(gameConfig.fromCfg(gameConfig.FIELDS.find((f) => f.cvar === "m_pitch"), "-0.03"), true);
});

test("GoldSrc: файлы в latin1 и CRLF, exec — в userconfig.cfg", () => {
  const root = tempInstall({
    "Half-Life/valve_WON/config.cfg": 'sensitivity "3"\r\n',
    "Half-Life/valve_WON/userconfig.cfg": "// café\r\nexec hlsr-personalization.cfg\r\n",
  });
  install.writeConfig(root, GOLDSRC, "hl1-won", "goldsrc", { sensitivity: 1.5, fps_max: 60, net_graph: 3 }, { language: null });
  assert.equal(read(root, "Half-Life/valve_WON/config.cfg", "latin1"), 'sensitivity "3"\r\n');
  assert.equal(read(root, "Half-Life/valve_WON/userconfig.cfg", "latin1"),
    '// café\r\nexec hlsr-personalization.cfg\r\n\r\n// ===== HLSR: settings =====\r\nsensitivity "1.5"\r\nfps_max "60"\r\nnet_graph "3"\r\n');
});

test("неверное значение и чужая переменная отклоняются, файлы не меняются", () => {
  const root = tempInstall({ "hl2/cfg/config.cfg": 'sensitivity "2"\n' });
  assert.throws(() => install.writeConfig(root, SOURCE, "hl2-4260", "source", { sensitivity: 500 }, { language: null }), { code: "INVALID_VALUE" });
  assert.throws(() => install.writeConfig(root, SOURCE, "hl2-4260", "source", { sv_cheats: 1 }, { language: null }), { code: "UNKNOWN_SETTING" });
  assert.throws(() => install.writeConfig(root, SOURCE, "hl2-4260", "source", { hud_fastswitch: "1; quit" }, { language: null }), { code: "INVALID_VALUE" });
  assert.equal(read(root, "hl2/cfg/config.cfg"), 'sensitivity "2"\n');
});

test("регистр имени файла на диске сохраняется", () => {
  const root = tempInstall({ "hl2/cfg/AutoExec.cfg": 'Sensitivity "2"\n' });
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { sensitivity: 3 }, { language: null });
  assert.equal(read(root, "hl2/cfg/AutoExec.cfg"), 'Sensitivity "3"\n');
  assert.deepEqual(fs.readdirSync(path.join(root, "hl2/cfg")), ["AutoExec.cfg"]);
});

test("HL1: WON.cfg не правится, его значение перекрывает строка в файле игрока", () => {
  const won = "// Don't mess with this (your run won't be legit otherwise)!\r\n\r\nclockwindow 0\r\nfps_max 100\r\n";
  const root = tempInstall({
    "Half-Life/valve_WON/config.cfg": 'fps_max "100"\r\nexec userconfig.cfg\r\n',
    "Half-Life/valve_WON/userconfig.cfg": "exec WON.cfg\r\nexec hlsr-personalization.cfg\r\n",
    "Half-Life/valve_WON/WON.cfg": won,
  });
  assert.equal(install.readConfig(root, GOLDSRC, "hl1-won", "goldsrc", { language: null }).sources.fps_max, "WON.cfg");
  install.writeConfig(root, GOLDSRC, "hl1-won", "goldsrc", { fps_max: 72 }, { language: null });
  assert.equal(read(root, "Half-Life/valve_WON/WON.cfg", "latin1"), won);
  assert.equal(fs.existsSync(path.join(root, "Half-Life/valve_WON/hlsr-config.cfg")), false);
  assert.equal(read(root, "Half-Life/valve_WON/userconfig.cfg", "latin1"),
    'exec WON.cfg\r\nexec hlsr-personalization.cfg\r\n\r\n// ===== HLSR: settings =====\r\nfps_max "72"\r\n');
  const after = install.readConfig(root, GOLDSRC, "hl1-won", "goldsrc", { language: null });
  assert.deepEqual([after.values.fps_max, after.sources.fps_max], ["72", "userconfig.cfg"]);
});

test("устаревший expectedSha256 даёт CONFIG_CHANGED, файл остаётся прежним", () => {
  const root = tempInstall({ "hl2/cfg/autoexec.cfg": 'fps_max 300\n' });
  const seen = install.readConfig(root, SOURCE, "hl2-4260", "source", { language: null });
  assert.match(seen.sha256, /^[0-9a-f]{64}$/);
  assert.equal(seen.playerFile, "autoexec.cfg", "имя файла игрока для окна: чип с ним не нужен");
  fs.writeFileSync(path.join(root, "hl2/cfg/autoexec.cfg"), 'fps_max 300\nr_lod 1\n');
  assert.throws(
    () => install.writeConfig(root, SOURCE, "hl2-4260", "source", { fps_max: 200 }, { language: null, expectedSha256: seen.sha256 }),
    { code: "CONFIG_CHANGED" }
  );
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"), 'fps_max 300\nr_lod 1\n');
  const fresh = install.readConfig(root, SOURCE, "hl2-4260", "source", { language: null });
  install.writeConfig(root, SOURCE, "hl2-4260", "source", { fps_max: 200 }, { language: null, expectedSha256: fresh.sha256 });
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"), 'fps_max 200\nr_lod 1\n');
});

test("M7: при запущенной игре «Конфиги» пишутся в файл игрока (игра его не переписывает)", () => {
  const root = tempInstall({ "hl2/cfg/config.cfg": 'fps_max "300"\n', "hl2/cfg/autoexec.cfg": "fps_max 300\n" });
  const controller = Object.create(require("../src/main/game-delivery").prototype);
  controller.configGameId = () => "hl2-4260";
  controller.installedPathOf = () => root;
  controller.preparePlayerConfig = () => null;
  controller.service = { isGameRunning: () => true };
  const result = controller.writeGameConfig("hl2-4260", null, { fps_max: 200 });
  assert.equal(result.status, "ok");
  assert.equal(read(root, "hl2/cfg/autoexec.cfg"), "fps_max 200\n");
  assert.equal(read(root, "hl2/cfg/config.cfg"), 'fps_max "300"\n');
});

test("переменная, заданная в файле игрока больше одного раза, попадает в shadowed", () => {
  const root = tempInstall({ "hl2/cfg/autoexec.cfg": 'fov 1\nfov 2\nfps_max 300\nvolume 0.5; volume 0.6\n' });
  assert.deepEqual(install.readConfig(root, SOURCE, "hl2-4260", "source", { language: null }).shadowed, ["fov", "volume"]);
  const single = tempInstall({ "hl2/cfg/autoexec.cfg": 'fov 1\n' });
  assert.deepEqual(install.readConfig(single, SOURCE, "hl2-4260", "source", { language: null }).shadowed, []);
});

test("Source: fps_max не больше 999", () => {
  const field = gameConfig.catalogFor("hl2-4260", "source").byCvar.get("fps_max");
  assert.equal(field.max, 999);
  assert.equal(gameConfig.toCfg(field, 1000), null);
  assert.equal(gameConfig.toCfg(field, 999), "999");
  assert.equal(gameConfig.importValues(gameConfig.catalogFor("hl2-4260", "source"), 'fps_max "1000"\n').values.fps_max, 999);
  assert.equal(gameConfig.catalogFor("hl1-won", "goldsrc").byCvar.get("fps_max").max, 100);
});

test("HL1: только разрешённое правилами WON", () => {
  const catalog = gameConfig.catalogFor("hl1-won", "goldsrc");
  assert.ok(!catalog.byCvar.has("brightness"), "brightness запрещён белым списком команд");
  assert.equal(catalog.byCvar.get("fps_max").max, 100, "fps_max не выше 100");
  assert.equal(gameConfig.toCfg(catalog.byCvar.get("fps_max"), 101), null);
});

test("значения из файлов персонализации на месте не правятся", () => {
  const root = tempInstall({
    "ghostingmod/cfg/config.cfg": "",
    "ghostingmod/cfg/autoexec.cfg": "exec hlsr-personalization.cfg\n",
    "ghostingmod/cfg/hlsr-personalization.cfg": "volume 0.1\n",
  });
  install.writeConfig(root, GHOSTING, "ghosting-renewed", "source", { volume: 50 }, { language: null });
  assert.equal(read(root, "ghostingmod/cfg/hlsr-personalization.cfg"), "volume 0.1\n");
  assert.equal(read(root, "ghostingmod/cfg/config.cfg"), "");
  assert.equal(read(root, "ghostingmod/cfg/autoexec.cfg"), 'exec hlsr-personalization.cfg\n\n// ===== HLSR: settings =====\nvolume "0.5"\n');
});

// Персонализация перекрывает свои переменные: в редакторе их быть не должно.
test("редактор не показывает переменные персонализации", () => {
  const cvarsOf = (text) => new Set(text.split(/\r?\n/).flatMap((line) => gameConfig.lineCommands(line).map((c) => c.args[0].value.toLowerCase())));
  const all = Object.fromEntries(ghostingSettings.FIELDS.map((field) => [field.key, field.default]));
  const ghosting = cvarsOf(ghostingPersonalization.toCfgLines({ extra: all }) + "\n" + ghostingSettings.serializeExtras(all));
  const goldsrc = cvarsOf(goldsrcPersonalization.toCfgLines({ bxt: goldsrcSettings.defaults() }));

  for (const field of gameConfig.catalogFor("ghosting-renewed", "source").fields)
    assert.ok(!ghosting.has(field.cvar.toLowerCase()), field.cvar);
  for (const field of gameConfig.catalogFor("hl1-won", "goldsrc").fields)
    assert.ok(!goldsrc.has(field.cvar.toLowerCase()), field.cvar);
});

test("у каждой настройки есть подписи на двух языках и верные пределы", () => {
  for (const field of gameConfig.FIELDS) {
    assert.ok(field.label.ru && field.label.en, field.cvar);
    if (field.type === "range") assert.ok(field.min <= field.default && field.default <= field.max, field.cvar);
    assert.notEqual(gameConfig.toCfg(field, gameConfig.defaultValue(field), undefined), null, field.cvar);
  }
});

// Steam на русском: GoldSrc ставит valve_WON_russian перед valve_WON и пишет
// туда config.cfg. Лаунчер читает и правит тот config.cfg, что видит игра.
test("GoldSrc: config.cfg из valve_WON_russian главнее, пока игра пишет туда", () => {
  const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
  const root = tempInstall({
    "Half-Life/valve_WON/config.cfg": 'sensitivity "3"\r\nbind "q" "+jump"\r\nm_pitch "0.022"\r\n',
    "Half-Life/valve_WON_russian/config.cfg": 'sensitivity "2"\r\nbind "q" "+duck"\r\nm_pitch "-0.022"\r\n',
  });
  const old = new Date(Date.now() - 60000);
  fs.utimesSync(path.join(root, "Half-Life/valve_WON/config.cfg"), old, old);

  assert.equal(install.readConfig(root, GOLDSRC, "hl1-won", "goldsrc", { language: null }).values.sensitivity, "2");
  assert.equal(gameBindsInstall.readInstallBinds(root, GOLDSRC.gameDirectory, "goldsrc").binds.q.command, "+duck");

  install.writeConfig(root, GOLDSRC, "hl1-won", "goldsrc", { sensitivity: 1.5 }, { language: null });
  assert.match(read(root, "Half-Life/valve_WON_russian/userconfig.cfg", "latin1"), /sensitivity "1\.5"\r\n/);
  assert.match(read(root, "Half-Life/valve_WON_russian/config.cfg", "latin1"), /^sensitivity "2"\r\n/);
  assert.match(read(root, "Half-Life/valve_WON/config.cfg", "latin1"), /^sensitivity "3"\r\n/);

  // Файл игрока лежит там же, где игра держит config.cfg: в языковой папке. Новых файлов лаунчера нет.
  const playerConfig = require("../src/main/game-delivery/player-config");
  const was = { personalization: {}, bxt: goldsrcSettings.defaults().bxt };
  const result = playerConfig.writePersonalization(root, "hl1-won", GOLDSRC.gameDirectory, was, { ...was, bxt: { ...was.bxt, fov: 100 } }, { language: null });
  assert.deepEqual(result.changed, ["default_fov"]);
  assert.equal(result.file, path.join(root, "Half-Life/valve_WON_russian/userconfig.cfg"));
  assert.match(read(root, "Half-Life/valve_WON_russian/userconfig.cfg", "latin1"), /default_fov "100"/);
  assert.equal(fs.existsSync(path.join(root, "Half-Life/valve_WON/userconfig.cfg")), false);
  assert.equal(fs.existsSync(path.join(root, "Half-Life/valve_WON/hlsr-personalization.cfg")), false);

  // Steam снова на английском: игра пишет config.cfg в valve_WON.
  fs.utimesSync(path.join(root, "Half-Life/valve_WON_russian/config.cfg"), old, old);
  fs.utimesSync(path.join(root, "Half-Life/valve_WON/config.cfg"), new Date(), new Date());
  assert.equal(install.readConfig(root, GOLDSRC, "hl1-won", "goldsrc", { language: null }).values.sensitivity, "3");
});

// Язык клиента Steam главнее времени записи: язык сменили, а игру после
// этого ещё не запускали — config.cfg прежнего языка новее, но игра его
// уже не прочитает.
test("GoldSrc: папку cfg выбирает язык Steam, время записи — только без него", () => {
  const cfgSearch = require("../src/main/game-delivery/cfg-search");
  const root = tempInstall({
    "Half-Life/valve_WON/config.cfg": "sensitivity 3\r\n",
    "Half-Life/valve_WON_russian/config.cfg": "sensitivity 2\r\n",
  });
  const old = new Date(Date.now() - 60000);
  fs.utimesSync(path.join(root, "Half-Life/valve_WON/config.cfg"), old, old);
  const mod = path.join(root, "Half-Life", "valve_WON");
  const ru = path.join(root, "Half-Life", "valve_WON_russian");
  const dirs = (language) => cfgSearch.searchDirs(root, GOLDSRC.gameDirectory, "goldsrc", { language });

  // Русский config.cfg новее, но Steam уже на английском.
  assert.deepEqual(dirs("english"), [mod]);
  assert.deepEqual(dirs("russian"), [ru, mod]);
  // Папки языка ещё нет: игра читает valve_WON.
  assert.deepEqual(dirs("german"), [mod]);
  // Язык не прочитан — по времени записи.
  assert.deepEqual(dirs(null), [ru, mod]);
  fs.utimesSync(path.join(root, "Half-Life/valve_WON_russian/config.cfg"), old, old);
  fs.utimesSync(path.join(root, "Half-Life/valve_WON/config.cfg"), new Date(), new Date());
  assert.deepEqual(dirs(null), [mod]);
  assert.deepEqual(dirs("russian"), [ru, mod]);
});

test("Импорт cfg во вкладку: последнее значение, цепочка exec, край диапазона и негодные значения", () => {
  const cfgImport = require("../src/shared/cfg-import");
  const catalog = gameConfig.catalogFor("hl1-won", "goldsrc");
  const built = cfgImport.buildImport({
    engine: "goldsrc",
    root: { name: "my.cfg", text: 'sensitivity 2\r\nexec "mouse.cfg"\r\nvolume "abc"\r\nm_pitch "-0.022" // invert\r\nunknown_cvar 5\r\n' },
    files: { "mouse.cfg": { name: "mouse.cfg", text: "sensitivity 99; alias x \"sensitivity 1\"\n" } },
  });
  const { values, skipped } = gameConfig.importValues(catalog, built.flatText);
  assert.equal(values.sensitivity, 10, "значение из файла цепочки, прижатое к краю окна; alias не выполняется");
  assert.equal(values.m_pitch, true, "отрицательный m_pitch — инверсия");
  assert.deepEqual(skipped, ["volume"]);
  assert.equal(Object.prototype.hasOwnProperty.call(values, "unknown_cvar"), false);
});

test("несколько выбранных cfg импортируются по порядку, как цепочка exec", () => {
  const cfgImport = require("../src/shared/cfg-import");
  const picked = [
    { name: "config.cfg", text: "sensitivity 2\nexec extra.cfg\n" },
    { name: "My Binds.cfg", text: "sensitivity 4\nbind f \"+use\"\n" },
  ];
  const { root, files } = cfgImport.rootOfPicked(picked);
  assert.equal(Object.keys(files).length, 2);
  // Выбранные файлы уже есть: окно выбора просится только для extra.cfg.
  const want = cfgImport.nextWanted({ engine: "goldsrc", root, files, missing: [], topLevelOnly: true });
  assert.equal(want && want.name, "extra.cfg");
  const built = cfgImport.buildImport({ engine: "goldsrc", root, files, missing: [want.key] });
  const { values } = gameConfig.importValues(gameConfig.catalogFor("hl1-won", "goldsrc"), built.flatText);
  assert.equal(Number(values.sensitivity), 4);
  // Один файл — без обёртки, как раньше.
  assert.deepEqual(cfgImport.rootOfPicked([picked[0]]).files, {});
});

// Вкладки «Конфига»: «Файлы cfg», «Проверка» (HL1), «Игра» — все разделы одной прокруткой по порядку.
test("«Конфиг»: вкладки и порядок разделов во вкладке «Игра» у HL1, Ghosting и HL2", () => {
  const { configTabs, GAME_ID } = require("../src/renderer/components/Launcher/config-tabs");
  const shape = (gameId, engine, opts) => configTabs(gameConfig.catalogFor(gameId, engine), opts)
    .map((tab) => [tab.id, tab.sections.map((section) => section.id)]);

  // HL1: у GoldSrc нет настроек раздела «Игра» — пустой раздел не показывается.
  assert.deepEqual(shape("hl1-won", "goldsrc", { files: true, check: true }),
    [["cfg-files", []], ["cfg-check", []], ["cfg-game", ["controls", "video", "audio"]]]);
  assert.deepEqual(shape("ghosting-renewed", "source", { files: true }),
    [["cfg-files", []], ["cfg-game", ["controls", "video", "audio", "game"]]]);
  assert.deepEqual(shape("hl2-4260", "source", { files: true }),
    [["cfg-files", []], ["cfg-game", ["controls", "video", "audio", "game"]]]);
  assert.deepEqual(shape("hl2-2707", "source-old", {}), [["cfg-game", ["controls", "video", "audio", "game"]]]);
  // Без каталога остаются вкладки окна; без них и каталога — пусто.
  assert.deepEqual(shape("x", "unknown", { files: true }), [["cfg-files", []]]);
  assert.deepEqual(configTabs(null), []);

  // Точка несохранённых правок у «Игры» считается по всем её настройкам, каждая — один раз.
  for (const [gameId, engine] of [["hl1-won", "goldsrc"], ["ghosting-renewed", "source"]]) {
    const catalog = gameConfig.catalogFor(gameId, engine);
    const game = configTabs(catalog).find((tab) => tab.id === GAME_ID);
    assert.deepEqual(game.fields.map((field) => field.cvar).sort(), catalog.fields.map((field) => field.cvar).sort(), gameId);
    assert.ok(game.title.ru === "Игра" && game.title.en === "Game");
  }

  // Редактор: разделы «Игры» подряд со своим заголовком и сбросом, якоря сверху, по умолчанию открыта «Игра».
  const editor = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameConfigEditor.vue"), "utf8");
  assert.match(editor, /<section v-for="section in activeSection\.sections"[^>]*class="gc-sec"/);
  assert.match(editor, /<h3 :id="'gc-sec-' \+ section\.id" class="gc-sec__title"/);
  assert.match(editor, /:disabled="disabled \|\| !sectionNonDefault\(section\)"[\s\S]*?@click="resetSection\(section\)"/);
  assert.match(editor, /v-for="\(column, columnIndex\) in columns\(section\)"/);
  assert.match(editor, /class="gc-jump__item" @click="jumpTo\(section\.id\)"/);
  assert.match(editor, /tabs\.find\(\(tab\) => tab\.id === this\.activeId\) \|\| tabs\.find\(\(tab\) => tab\.id === GAME_ID\)/);
  for (const lang of ["ru", "en"])
    assert.match(fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8"), /"#L_GS_CFG_JUMP":/, lang);
});
