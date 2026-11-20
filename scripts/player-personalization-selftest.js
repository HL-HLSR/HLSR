"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");

const playerConfig = require("../src/main/game-delivery/player-config");
const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const ghostingPersonalization = require("../src/main/game-delivery/ghosting-personalization");
const goldsrcPersonalization = require("../src/main/game-delivery/goldsrc-personalization");
const launchArgs = require("../src/main/game-delivery/launch-args");
const schema = require("../src/shared/ghosting-settings");

const HL1 = "Half-Life/valve_WON";
const GHOST = "ghostingmod";
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };

function setup(t, gameId, text) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-player-personalization-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const dir = gameId === "hl1-won" ? path.join(root, "Half-Life", "valve_WON") : path.join(root, GHOST, "cfg");
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, gameId === "hl1-won" ? "userconfig.cfg" : "autoexec.cfg");
  if (text !== undefined) fs.writeFileSync(file, text, "latin1");
  return { root, dir, file, gameId, gameDirectory: gameId === "hl1-won" ? HL1 : GHOST };
}

const read = (env) => fs.readFileSync(env.file, "latin1");
const write = (env, prev, next, options = {}) =>
  playerConfig.writePersonalization(env.root, env.gameId, env.gameDirectory, prev, next, { ...EN, ...options });
const load = (env, stored) => playerConfig.readPersonalization(env.root, env.gameId, env.gameDirectory, stored, EN);
const defaults = () => launchArgs.defaultSettings();
const withBxt = (settings, bxt) => ({ ...settings, bxt: { ...settings.bxt, ...bxt } });
const withLook = (settings, look) => ({ ...settings, personalization: { ...settings.personalization, ...look } });

const HEADERS = "// ===== HLSR: settings =====\r\nfps_max 100\r\n\r\n// ===== HLSR: personalization =====\r\n\r\n// ===== HLSR: binds =====\r\n\r\n// ===== Your own =====\r\n";

test("HL1: FOV 100 в настройках — в файл добавлена одна строка default_fov, больше ничего", (t) => {
  const env = setup(t, "hl1-won", HEADERS);
  const result = write(env, defaults(), withBxt(defaults(), { fov: 100 }));
  assert.deepEqual(result.changed, ["default_fov"]);
  assert.equal(result.file, env.file);
  assert.equal(
    read(env),
    HEADERS.replace("personalization =====\r\n", 'personalization =====\r\ndefault_fov "100"\r\n')
  );
  assert.equal(fs.existsSync(path.join(env.dir, "hlsr-personalization.cfg")), false, "снимок не создаётся сам");
});

test("HL1: без изменений файл не пишется, байты 0x80-0xFF на месте", (t) => {
  const before = "// \xcf\xf0\xe8\xe2\xe5\xf2\r\ndefault_fov 115 // \xe2\xee\xf2\r\n";
  const env = setup(t, "hl1-won", before);
  const shown = load(env, defaults());
  const result = write(env, defaults(), shown);
  assert.deepEqual(result.changed, []);
  assert.equal(read(env), before);
  // Правка одной строки не меняет остальные байты.
  write(env, defaults(), withBxt(shown, { fov: 100 }));
  assert.equal(read(env), before.replace("default_fov 115", "default_fov 100"));
});

test("HL1: FOV, написанный игроком, читается; сохранение прицела строку FOV не трогает", (t) => {
  const env = setup(t, "hl1-won", HEADERS + 'default_fov "115"\r\n');
  const stored = defaults();
  const shown = load(env, stored);
  assert.equal(shown.bxt.fov, 115);
  assert.equal(stored.bxt.fov, 90, "хранилище не меняется");

  // Окно открыто с shown; в хранилище всё ещё 90.
  const next = withLook(shown, { crossEnabled: true, crossSize: 14 });
  const result = write(env, stored, next);
  assert.ok(!result.changed.includes("default_fov"), result.changed.join());
  assert.ok(result.changed.includes("bxt_cross_size"));
  const after = read(env);
  assert.equal(after.match(/default_fov/g).length, 1);
  assert.match(after, /default_fov "115"\r\n/);
  assert.match(after, /bxt_cross_size "14"/);
});

test("HL1: цвет HUD с прозрачностью читается обратно без изменений", (t) => {
  const env = setup(t, "hl1-won", HEADERS);
  const next = withLook(defaults(), { hudEnabled: true, hudColor: { r: 0, g: 200, b: 255 }, hudAlpha: 200 });
  write(env, defaults(), next);
  assert.match(read(env), /bxt_hud_color "0 157 200"/);
  assert.deepEqual(load(env, next), next, "записанное нами не подменяет настройки");
});

test("HL1: ручное значение прицела подхватывается, HUD без строки в файле - по умолчанию", (t) => {
  const env = setup(t, "hl1-won", HEADERS + 'bxt_cross_size "22"\r\nbxt_cross "1"\r\n');
  const stored = withLook(defaults(), { hudEnabled: true, hudColor: { r: 100, g: 100, b: 100 }, hudAlpha: 128 });
  const shown = load(env, stored);
  assert.equal(shown.personalization.crossSize, 22);
  assert.equal(shown.personalization.crossEnabled, true);
  assert.equal(shown.personalization.hudEnabled, false);
});

test("Ghosting: размер прицела — одна строка gh_cross_size; ручное значение читается", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS);
  const result = write(env, defaults(), withLook(defaults(), { crossSize: 12 }));
  assert.deepEqual(result.changed, ["gh_cross_size"]);
  assert.equal(read(env), HEADERS.replace("personalization =====\r\n", 'personalization =====\r\ngh_cross_size "12"\r\n'));

  fs.writeFileSync(env.file, read(env).replace('gh_cross_size "12"', 'gh_cross_size "30"'), "latin1");
  const stored = withLook(defaults(), { crossSize: 12, crossPreset: "dot" });
  const shown = load(env, stored);
  assert.equal(shown.personalization.crossSize, 30);
  assert.equal(shown.personalization.crossPreset, "custom", "прицел правили руками");

  // Цвет HUD после этого: строка размера прицела на месте.
  write(env, stored, withLook(shown, { hudColor: { r: 1, g: 2, b: 3 } }));
  assert.match(read(env), /gh_cross_size "30"/);
  assert.match(read(env), /gh_hud_color "1 2 3"/);
});

test("C3: gh_name кириллицей — в autoexec байтами UTF-8 (как 0.4.8), окно читает имя обратно; повтор файл не трогает", (t) => {
  const own = Buffer.from("// Мой autoexec\r\n" + HEADERS, "utf8");
  const env = setup(t, "ghosting-renewed");
  fs.writeFileSync(env.file, own);
  const next = withLook(defaults(), { extra: { ...defaults().personalization.extra, gh_name: "Влад" } });
  assert.deepEqual(write(env, defaults(), next).changed, ["gh_name"]);
  const bytes = fs.readFileSync(env.file);
  assert.ok(bytes.includes(Buffer.from('gh_name "Влад"', "utf8")));
  assert.deepEqual(bytes.subarray(0, Buffer.byteLength("// Мой autoexec\r\n")), Buffer.from("// Мой autoexec\r\n", "utf8"));
  const shown = load(env, defaults());
  assert.equal(shown.personalization.extra.gh_name, "Влад");
  assert.deepEqual(write(env, defaults(), shown).changed, [], "то же имя — без записи");
  assert.deepEqual(fs.readFileSync(env.file), bytes);
});

test("M5: «Сбросить» в «Персонализации» пишет умолчания явными строками; без сброса то же сохранение файл не трогает", (t) => {
  const GameDelivery = require("../src/main/game-delivery");
  const GameSettingsStore = require("../src/main/game-delivery/GameSettingsStore");
  for (const reset of [false, true]) {
    const env = setup(t, "hl1-won", HEADERS);
    // FOV 110 сохранила сама игра в config.cfg; в файле игрока строки FOV нет.
    fs.writeFileSync(path.join(env.dir, "config.cfg"), 'default_fov "110"\r\nexec userconfig.cfg\r\n', "latin1");
    const controller = Object.create(GameDelivery.prototype);
    const store = new GameSettingsStore({ file: path.join(env.root, "game-settings.json") });
    controller.ensureSettingsStore = () => store;
    controller.resolveGameId = (id) => id;
    controller.gameLanguages = () => [];
    controller.service = { installService: { readInstalled: () => ({ installPath: env.root }) }, isGameRunning: () => false };
    const opened = controller.gameSettings("get", "hl1-won");
    assert.equal(opened.settings.bxt.fov, 90, "окно показывает умолчание игры");
    // Черновик после resetGame: персонализация по умолчанию, паки и BXT сброшены.
    const look = { personalization: defaults().personalization, packs: null, bxt: null };
    const options = { configSha256: opened.configSha256, ...(reset ? { reset: true } : {}) };
    assert.equal(controller.gameSettings("save", "hl1-won", look, "personalization", options).status, "ok");
    if (!reset) {
      assert.equal(read(env), HEADERS, "без сброса — только отличия, их нет");
      continue;
    }
    const pairs = playerConfig.personalizationPairs("hl1-won", launchArgs.normalizeSettings({ ...defaults(), ...look }).settings);
    assert.ok(Object.keys(pairs).length > 5);
    const doc = require("../src/shared/cfg-document").parse(read(env));
    for (const [name, value] of Object.entries(pairs))
      assert.equal((require("../src/shared/cfg-document").get(doc, `cvar:${name}`) || {}).value, value, `явная строка ${name}`);
    assert.equal(require("../src/shared/cfg-document").get(doc, "cvar:default_fov").value, "90");
  }
});

test("Ghosting: правка не прицела не делает пресет прицела «своим»", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS + 'hud_quickinfo "0"\r\n');
  const stored = withLook(defaults(), { crossPreset: "classic" });
  const shown = load(env, stored);
  assert.equal(shown.personalization.hideQuickInfo, true);
  assert.equal(shown.personalization.crossPreset, "classic");
});

test("Ghosting: значения, которых нет в файле, показываются как умолчания игры; файлы спрея и YouTube - из настроек", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS);
  const stored = withLook(defaults(), { crossSize: 12, extra: { viewmodel_fov: 80, gh_spray_file: "custom/a.png" } });
  const shown = load(env, stored).personalization;
  assert.equal(shown.crossSize, defaults().personalization.crossSize);
  assert.equal(shown.extra.viewmodel_fov, undefined);
  assert.equal(shown.extra.gh_spray_file, "custom/a.png");
  assert.deepEqual(load(env, defaults()), defaults(), "настройки по умолчанию остаются как есть");
});

test("HL1: игрок удалил строку default_fov - окно показывает FOV по умолчанию, а не из settings.json", (t) => {
  const env = setup(t, "hl1-won", HEADERS + "bxt_cross_size 22\r\n");
  const stored = withBxt(withLook(defaults(), { crossSize: 22 }), { fov: 110 });
  const shown = load(env, stored);
  assert.equal(shown.bxt.fov, defaults().bxt.fov);
  assert.equal(shown.personalization.crossSize, 22);
});

const SPRAY = "custom/a.png";
const CHAT_URL = "https://www.youtube.com/watch?v=abcdefghijk";

test("Ghosting: убрать спрей - gh_spray_clear в блоке extras, gh_spray_set не остаётся нигде", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS + `gh_spray_set "old/x.png"\r\n`);
  const withSpray = withLook(defaults(), { extra: { gh_spray_file: SPRAY } });
  write(env, defaults(), withSpray);
  const doc = read(env);
  assert.equal(doc.match(/gh_spray_set/g).length, 1, "строка 0.4.8 вне блока убрана");
  assert.ok(doc.indexOf("HLSR script ghosting-extras begin") < doc.indexOf("gh_spray_set"));
  assert.equal(load(env, withSpray).personalization.extra.gh_spray_file, SPRAY);

  const removed = withLook(defaults(), { extra: { gh_spray_file: "" } });
  const result = write(env, withSpray, removed);
  assert.ok(result.changed.includes(playerConfig.EXTRAS_BLOCK));
  assert.doesNotMatch(read(env), /gh_spray_set/);
  assert.match(read(env), /^gh_spray_clear\r\n/m);
  assert.equal(load(env, removed).personalization.extra.gh_spray_file, "");
});

test("Ghosting: «Сбросить» убирает спрей и чат из файла", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS);
  const extra = { gh_spray_file: SPRAY, gh_chat: true, gh_chat_channel: "runner", gh_chat_connect: CHAT_URL };
  const was = withLook(defaults(), { extra });
  write(env, defaults(), was);
  assert.match(read(env), /gh_chat_connect "runner"/);
  assert.match(read(env), /gh_chat_connect "https:\/\/www\.youtube\.com/);
  const reset = withLook(defaults(), { extra: schema.resetExtras(extra) });
  write(env, was, reset);
  const doc = read(env);
  assert.doesNotMatch(doc, /gh_spray_set|gh_chat_connect/);
  assert.match(doc, /gh_spray_clear/);
  assert.match(doc, /gh_chat_disconnect youtube/);
  assert.match(doc, /gh_chat_disconnect twitch/);
});

test("Ghosting: оба чата включены, меняется только Twitch - изменение доходит до файла", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS);
  const first = withLook(defaults(), { extra: { gh_chat: true, gh_chat_channel: "runner", gh_chat_connect: CHAT_URL } });
  write(env, defaults(), first);
  const second = withLook(defaults(), { extra: { gh_chat: true, gh_chat_channel: "other", gh_chat_connect: CHAT_URL } });
  const result = write(env, first, second);
  assert.ok(result.changed.includes("gh_chat_channel") && result.changed.includes(playerConfig.EXTRAS_BLOCK));
  const doc = read(env);
  assert.match(doc, /gh_chat_connect "other"/);
  assert.doesNotMatch(doc, /gh_chat_connect "runner"/);
  assert.match(doc, /gh_chat_connect "https:\/\/www\.youtube\.com/);
  assert.equal(load(env, second).personalization.extra.gh_chat_channel, "other");
});

test("Ghosting: extras (FOV) читаются из файла игрока", (t) => {
  const env = setup(t, "ghosting-renewed", HEADERS + 'fov_desired "100"\r\n');
  const shown = load(env, withLook(defaults(), { extra: { fov_desired: 90 } }));
  assert.equal(shown.personalization.extra.fov_desired, 100);
});

test("importFromConfigCfg и importExtrasFromConfigCfg читают переданный текст", () => {
  const text = 'gh_cross_size "31"\r\nviewmodel_fov "70"\r\n';
  assert.equal(ghostingPersonalization.importFromConfigCfg("/нет/такой/папки", { text }).crossSize, 31);
  assert.deepEqual(ghostingPersonalization.importExtrasFromConfigCfg("/нет/такой/папки", { text }), { viewmodel_fov: 70 });
  assert.equal(ghostingPersonalization.importExtrasFromConfigCfg("/нет/такой/папки", { text: "// пусто" }), null);
});

test("файл изменён между чтением и записью: CONFIG_CHANGED, файл не тронут", (t) => {
  const env = setup(t, "hl1-won", HEADERS);
  const seen = playerConfig.read(env.root, env.gameId, env.gameDirectory, EN);
  fs.writeFileSync(env.file, read(env) + "// рука\r\n", "latin1");
  const before = read(env);
  assert.throws(
    () => write(env, defaults(), withBxt(defaults(), { fov: 100 }), { expectedSha256: seen.sha256 }),
    (error) => error.code === "CONFIG_CHANGED"
  );
  assert.equal(read(env), before);
});

test("файла игрока ещё нет: появляется раздел personalization с изменёнными строками", (t) => {
  const env = setup(t, "hl1-won");
  write(env, defaults(), withBxt(defaults(), { fov: 100 }));
  assert.equal(read(env), '// ===== HLSR: personalization =====\r\ndefault_fov "100"\r\n');
});

test("снимок для скриптов гайда: есть файл или ссылка на него - переписывается, иначе не создаётся", (t) => {
  const env = setup(t, "hl1-won", HEADERS);
  const snapshot = path.join(env.dir, "hlsr-personalization.cfg");
  const next = withBxt(defaults(), { fov: 100 });
  write(env, defaults(), next);
  assert.equal(fs.existsSync(snapshot), false);

  fs.writeFileSync(snapshot, "// старый снимок\r\n", "latin1");
  write(env, next, withBxt(defaults(), { fov: 101 }));
  assert.equal(fs.readFileSync(snapshot, "latin1"), goldsrcPersonalization.toCfgLines(withBxt(defaults(), { fov: 101 })));
  assert.doesNotMatch(read(env), /exec hlsr-personalization/, "в файл игрока exec не добавляется");

  // Файла нет, но блок скрипта зовёт его.
  fs.rmSync(snapshot);
  const block = "// HLSR script deffov begin\r\nalias deffov \"exec hlsr-personalization.cfg\"\r\n// HLSR script deffov end\r\n";
  fs.writeFileSync(env.file, read(env) + block, "latin1");
  write(env, withBxt(defaults(), { fov: 101 }), withBxt(defaults(), { fov: 102 }));
  assert.equal(fs.readFileSync(snapshot, "latin1"), goldsrcPersonalization.toCfgLines(withBxt(defaults(), { fov: 102 })));
});

test("applyBinds с previousSettings: бинды и персонализация одним чтением и одной записью", (t) => {
  const env = setup(t, "hl1-won", HEADERS);
  let reads = 0;
  let writes = 0;
  const real = fs;
  const counting = new Proxy(real, {
    get(target, key) {
      if (key === "readFileSync") return (...args) => { if (String(args[0]) === env.file) reads += 1; return target.readFileSync(...args); };
      if (key === "renameSync") return (...args) => { if (String(args[1]) === env.file) writes += 1; return target.renameSync(...args); };
      return target[key];
    },
  });
  const next = withBxt(defaults(), { fov: 100 });
  gameBindsInstall.applyBinds(env.root, "hl1-won", HL1, { f: { command: "+duck", base: "" } }, {
    ...EN, fs: counting, settings: next, previousSettings: defaults(),
  });
  // Одно чтение и одна запись; второе чтение - проверка отпечатка внутри write.
  assert.ok(reads <= 2, `чтений: ${reads}`);
  assert.equal(writes, 1);
  assert.match(read(env), /default_fov "100"/);
  assert.match(read(env), /bind "f" "\+duck"/);
});
