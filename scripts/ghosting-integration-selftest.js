"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const vm = require("node:vm");
const GameSettingsStore = require("../src/main/game-delivery/GameSettingsStore");
const personalization = require("../src/main/game-delivery/ghosting-personalization");
const schema = require("../src/shared/ghosting-settings");

function fixture(t, dialog) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-settings-integration-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const cfgDir = path.join(root, "ghostingmod/cfg");
  fs.mkdirSync(cfgDir, { recursive: true });
  const store = new GameSettingsStore({ file: path.join(root, "game-settings.json") });
  const exported = { exports: {} };
  const source = fs.readFileSync(path.join(__dirname, "../src/main/game-delivery/index.js"), "utf8");
  vm.runInNewContext(source, {
    module: exported,
    process,
    require(name) {
      if (name === "electron") return { ipcMain: {}, dialog: dialog || {} };
      if (name === "electron-log") return { warn() {}, info() {} };
      if (name === "./ghosting-personalization") return personalization;
      if (name === "./personalization-code") return require("../src/main/game-delivery/personalization-code");
      if (name === "./ghosting-assets") return require("../src/main/game-delivery/ghosting-assets");
      if (name === "../../shared/ghosting-settings") return schema;
      if (name === "./launch-args") return require("../src/main/game-delivery/launch-args");
      if (name === "../../shared/links") return { urlFor() { return ""; } };
      if (name === "./game-binds-install") return require("../src/main/game-delivery/game-binds-install");
      if (name === "./player-config") return require("../src/main/game-delivery/player-config");
      if (name === "../../shared/game-binds") return require("../src/shared/game-binds");
      if (name === "../../shared/bind-presets") return require("../src/shared/bind-presets");
      if (name === "./manifests") return require("../src/main/game-delivery/manifests");
      if (name === "./custom-packs") return require("../src/main/game-delivery/custom-packs");
      return {};
    },
  }, { filename: "game-delivery/index.js" });
  const controller = Object.create(exported.exports.prototype);
  controller.ensureSettingsStore = () => store;
  controller.resolveGameId = (id) => id === "218" ? "ghosting-renewed" : id === "70" ? "hl1-won" : id;
  controller.gameLanguages = () => [];
  controller.service = { installService: { readInstalled: () => ({ installPath: root }) } };
  return { root, cfgDir, store, controller };
}

const playerFile = (cfgDir) => path.join(cfgDir, "autoexec.cfg");

test("значения из файла игрока показываются в окне; открытие и сохранение их не перезаписывают", (t) => {
  const { cfgDir, store, controller } = fixture(t);
  store.save("ghosting-renewed", { personalization: { crossEnabled: true, crossSize: 23 } });
  fs.writeFileSync(path.join(cfgDir, "config.cfg"), 'viewmodel_fov "50"\n');
  fs.writeFileSync(playerFile(cfgDir), 'viewmodel_fov "76"\r\ngh_cross_size "3"\r\n');
  const result = controller.gameSettings("get", "218");
  assert.equal(result.status, "ok");
  assert.equal(result.settings.personalization.crossSize, 3, "файл игрока главнее сохранённого");
  assert.equal(result.settings.personalization.extra.viewmodel_fov, 76);
  assert.equal(store.get("ghosting-renewed").personalization.crossSize, 23, "открытие окна ничего не сохраняет");
  const saved = controller.gameSettings("save", "218", result.settings);
  assert.equal(saved.status, "ok");
  const cfg = fs.readFileSync(playerFile(cfgDir), "utf8");
  assert.equal(cfg.match(/gh_cross_size/g).length, 1);
  assert.match(cfg, /gh_cross_size "3"/);
  assert.match(cfg, /viewmodel_fov "76"/);
  assert.equal(fs.existsSync(path.join(cfgDir, "hlsr-personalization.cfg")), false);
  fs.writeFileSync(playerFile(cfgDir), cfg.replace('viewmodel_fov "76"', 'viewmodel_fov "93"'));
  assert.equal(controller.gameSettings("get", "218").settings.personalization.extra.viewmodel_fov, 93, "правка руками видна сразу");
});

test("sha файла игрока: get отдаёт его, save и reset со старым sha - CONFIG_CHANGED, файл и хранилище прежние", (t) => {
  const { cfgDir, store, controller } = fixture(t);
  const file = playerFile(cfgDir);
  const crypto = require("node:crypto");
  const shaOf = () => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  fs.writeFileSync(file, 'gh_cross_size "3"\r\n');
  store.save("ghosting-renewed", { personalization: { crossSize: 3 } });
  const opened = controller.gameSettings("get", "218");
  assert.equal(opened.configSha256, shaOf());

  // Игрок правит файл руками, пока окно открыто; в окне эту настройку не трогали.
  fs.writeFileSync(file, 'gh_cross_size "3"\r\nviewmodel_fov "70"\r\n');
  const manual = fs.readFileSync(file, "utf8");
  const draft = { ...opened.settings, personalization: { ...opened.settings.personalization, crossSize: 14 } };
  const rejected = controller.gameSettings("save", "218", draft, undefined, { configSha256: opened.configSha256 });
  assert.equal(rejected.status, "error");
  assert.equal(rejected.error.code, "CONFIG_CHANGED");
  assert.equal(fs.readFileSync(file, "utf8"), manual);
  assert.equal(store.get("ghosting-renewed").personalization.crossSize, 3);
  assert.equal(controller.gameSettings("reset", "218", undefined, undefined, { configSha256: opened.configSha256 }).error.code, "CONFIG_CHANGED");
  assert.equal(fs.readFileSync(file, "utf8"), manual);

  // Окно перечиталось: свежий sha проходит и возвращается новый.
  const reopened = controller.gameSettings("get", "218");
  assert.equal(reopened.settings.personalization.extra.viewmodel_fov, 70);
  const saved = controller.gameSettings("save", "218", { ...reopened.settings,
    personalization: { ...reopened.settings.personalization, crossSize: 14 } }, undefined, { configSha256: reopened.configSha256 });
  assert.equal(saved.status, "ok");
  assert.equal(saved.configSha256, shaOf());
  assert.match(fs.readFileSync(file, "utf8"), /viewmodel_fov "70"/);
  // Старый вызов без sha работает как раньше.
  assert.equal(controller.gameSettings("save", "218", saved.settings).status, "ok");
});
test("reset пишет значения по умолчанию в файл игрока и не оживляет config.cfg", (t) => {
  const { cfgDir, controller } = fixture(t);
  controller.gameSettings("save", "218", { personalization: { extra: { viewmodel_fov: 91, gh_autosave: false } } });
  assert.match(fs.readFileSync(playerFile(cfgDir), "utf8"), /viewmodel_fov "91"/);
  const result = controller.gameSettings("reset", "218");
  assert.equal(result.status, "ok");
  assert.equal(result.settings.personalization.extra.viewmodel_fov, 62);
  assert.equal(result.settings.personalization.extra.gh_autosave, true);
  assert.equal(result.settings.personalization.extra.gh_keyshud, undefined, "unmanaged settings stay unmanaged");
  assert.match(fs.readFileSync(playerFile(cfgDir), "utf8"), /viewmodel_fov "62"/);
  fs.writeFileSync(path.join(cfgDir, "config.cfg"), 'viewmodel_fov "91"\n');
  assert.equal(controller.gameSettings("get", "218").settings.personalization.extra.viewmodel_fov, 62);
});

test("reset: файл игрока не записался - ответ с ошибкой, хранилище прежнее", (t) => {
  const { store, controller } = fixture(t);
  store.save("ghosting-renewed", { personalization: { crossSize: 17 } });
  const install = require("../src/main/game-delivery/game-binds-install");
  const original = install.applyBinds;
  install.applyBinds = () => { throw Object.assign(new Error("changed"), { code: "CONFIG_CHANGED" }); };
  t.after(() => { install.applyBinds = original; });
  const result = controller.gameSettings("reset", "218");
  assert.equal(result.status, "error");
  assert.equal(result.error.code, "CONFIG_CHANGED");
  assert.equal(store.get("ghosting-renewed").personalization.crossSize, 17);
});

test("HL1: FOV в настройках - одна строка в userconfig.cfg; код после правки FOV руками содержит новый FOV", (t) => {
  const { root, store, controller } = fixture(t);
  const modDir = path.join(root, "Half-Life", "valve_WON");
  fs.mkdirSync(modDir, { recursive: true });
  const file = path.join(modDir, "userconfig.cfg");
  const original = "// ===== HLSR: personalization =====\r\n\r\n// ===== Your own =====\r\n";
  fs.writeFileSync(file, original);
  const saved = controller.gameSettings("save", "70", { bxt: { fov: 100 } }, "personalization");
  assert.equal(saved.status, "ok");
  assert.equal(fs.readFileSync(file, "utf8"), original.replace("=====\r\n\r\n", '=====\r\ndefault_fov "100"\r\n\r\n'));

  // Игрок правит FOV руками; окно открыто с прежним черновиком, а код берёт файл.
  fs.writeFileSync(file, fs.readFileSync(file, "utf8").replace('default_fov "100"', 'default_fov "115"'));
  const draft = store.get("hl1-won");
  assert.equal(controller.gameSettings("get", "70").settings.bxt.fov, 115);
  const exported = controller.personalizationCode("export", "70", draft);
  assert.equal(controller.personalizationCode("import", "70", exported.code).look.bxt.fov, 115);
  // Несохранённая правка в черновике главнее файла.
  const edited = controller.personalizationCode("export", "70", { ...draft, bxt: { ...draft.bxt, fov: 80 } });
  assert.equal(controller.personalizationCode("import", "70", edited.code).look.bxt.fov, 80);
  // Сохранение прицела строку FOV не трогает.
  const shown = controller.gameSettings("get", "70").settings;
  controller.gameSettings("save", "70", { personalization: { ...shown.personalization, crossEnabled: true, crossSize: 14 }, bxt: shown.bxt, packs: shown.packs }, "personalization");
  const after = fs.readFileSync(file, "utf8");
  assert.equal(after.match(/default_fov/g).length, 1);
  assert.match(after, /default_fov "115"/);
  assert.match(after, /bxt_cross_size "14"/);
});

test("код лишь обновляет черновик, сохранение записывает импорт в файл игрока и сохраняет запуск и тему", (t) => {
  const { root, cfgDir, store, controller } = fixture(t);
  store.save("ghosting-renewed", { launchArgs: "-console", rinput: false, language: "en", personalization: { extra: { gh_timer: true, gh_keyshud: true } } });
  const preferences = path.join(root, "renderer-preferences.json");
  fs.writeFileSync(preferences, '{"launcherTheme":"xen"}');
  const file = playerFile(cfgDir);
  fs.writeFileSync(file, 'gh_timer "1"\n');
  const beforeStore = fs.readFileSync(store.file, "utf8");
  const exported = controller.personalizationCode("export", "218", { personalization: {
    hudEnabled: true, hudColor: { r: 10, g: 160, b: 180 }, extra: { gh_health_x: 111, viewmodel_fov: 81, gh_timer: false },
  } });
  assert.equal(exported.status, "ok");
  const imported = controller.personalizationCode("import", "218", exported.code);
  assert.equal(imported.status, "ok");
  assert.equal(controller.personalizationCode("import", "70", exported.code).error.code, "PERSONALIZATION_CODE_WRONG_GAME");
  assert.equal(fs.readFileSync(file, "utf8"), 'gh_timer "1"\n', "импорт ещё не меняет игру");
  assert.equal(fs.readFileSync(store.file, "utf8"), beforeStore);
  const saved = controller.gameSettings("save", "218", imported.look, "personalization");
  assert.equal(saved.status, "ok");
  assert.equal(saved.settings.launchArgs, "-console");
  assert.equal(saved.settings.rinput, false);
  assert.equal(saved.settings.language, "en");
  const cfg = fs.readFileSync(file, "utf8");
  assert.match(cfg, /gh_health_x "111"/);
  assert.match(cfg, /viewmodel_fov "81"/);
  assert.match(cfg, /^gh_timer "0"$/m, "выключенные у автора элементы не остаются включёнными у получателя");
  assert.equal(cfg.match(/^gh_timer "/gm).length, 1, "строка игрока правится на месте");
  assert.match(cfg, /gh_keyshud "0"/, "отсутствующие в черновике автора поля имеют его значения по умолчанию");
  assert.equal(fs.readFileSync(preferences, "utf8"), '{"launcherTheme":"xen"}');
});
test("персонализация сохраняет актуальные параметры и бинды, даже если черновик загружен раньше", (t) => {
  const { store, controller } = fixture(t);
  const look = require("../src/main/game-delivery/personalization-code").decode("218",
    require("../src/main/game-delivery/personalization-code").encode("218", { personalization: { crossSize: 18 } }));
  store.save("ghosting-renewed", { launchArgs: "-console", rinput: false, language: "en",
    binds: { space: { command: "+duck", base: "+jump" } }, width: 1920, height: 1080 });
  const before = store.get("ghosting-renewed");
  const saved = controller.gameSettings("save", "218", Object.assign({}, look, { launchArgs: "", rinput: true, binds: {} }), "personalization");
  assert.equal(saved.status, "ok");
  assert.equal(saved.settings.personalization.crossSize, 18);
  assert.deepEqual(JSON.parse(JSON.stringify(Object.assign({}, saved.settings, { personalization: before.personalization }))), before);
  assert.equal(controller.gameSettings("save", "218", {}, "unknown").error.code, "SETTINGS_SCOPE_INVALID");
  const hl1 = { personalization: { hudEnabled: true }, packs: { hud: "ag-hud", crosshair: "crosshair-changer", changerShape: "3px", changerColor: "pink" }, bxt: { timer: true, fov: 110 } };
  store.save("hl1-won", { launchArgs: "-console", rinputMenuFix: true });
  const savedHl1 = controller.gameSettings("save", "70", hl1, "personalization");
  assert.equal(savedHl1.status, "ok");
  assert.equal(savedHl1.settings.packs.hud, "ag-hud");
  assert.equal(savedHl1.settings.bxt.fov, 110);
  assert.equal(savedHl1.settings.launchArgs, "-console");
  assert.equal(savedHl1.settings.rinputMenuFix, true);
});

test("preview assets are served only for imported Ghosting assets", (t) => {
  const { importAsset } = require("../src/main/game-delivery/ghosting-assets");
  const { root, controller } = fixture(t);
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aGhsAAAAASUVORK5CYII=", "base64");
  const source = path.join(root, "my spray.png");
  fs.writeFileSync(source, png);
  const imported = importAsset(root, source, "spray");
  const preview = controller.readPersonalizationPreviewAsset("218", { kind: "spray", value: imported.value });
  assert.equal(preview.status, "ok");
  assert.equal(preview.dataUrl, "data:image/png;base64," + png.toString("base64"));
  assert.equal(controller.readPersonalizationPreviewAsset("70", { kind: "spray", value: imported.value }).code, "INVALID_ASSET");
  assert.equal(controller.readPersonalizationPreviewAsset("218", { kind: "executable", value: "x" }).code, "INVALID_ASSET");
  assert.equal(controller.readPersonalizationPreviewAsset("218", null).code, "INVALID_ASSET");
  assert.equal(controller.readPersonalizationPreviewAsset("218", { kind: "spray", value: "../outside.png" }).code, "ASSET_INVALID_PATH");
  assert.equal(controller.readPersonalizationPreviewAsset("218", { kind: "font", value: "Impact" }).code, "ASSET_READ_FAILED");
  controller.service = { installService: { readInstalled: () => null } };
  assert.equal(controller.readPersonalizationPreviewAsset("218", { kind: "spray", value: imported.value }).code, "NOT_INSTALLED");
});

test("asset picker rejects unsupported games and cancellation leaves installation untouched", async (t) => {
  let calls = 0;
  const { root, controller } = fixture(t, { async showOpenDialog() { calls++; return { canceled: true, filePaths: [] }; } });
  assert.equal((await controller.pickPersonalizationAsset("70", "font")).status, "error");
  assert.equal((await controller.pickPersonalizationAsset("218", "executable")).status, "error");
  assert.equal(calls, 0);
  assert.equal((await controller.pickPersonalizationAsset("218", "font")).status, "canceled");
  assert.equal(calls, 1);
  assert.equal(fs.existsSync(path.join(root, "ghostingmod/resource")), false);
});

test("LiveSplit on Linux runs in the prefix of the game chosen in Proton settings", (t) => {
  const { controller } = fixture(t);
  let running = null;
  controller.service = { processService: { anyRunning: () => running } };
  controller.lastSelectedGameId = "ghosting-renewed";
  controller.resolveGameId = (id) => ({ 70: "hl1-won", 218: "ghosting-renewed", 220: "hl2-2707" })[id] || null;

  assert.equal(controller.toolGameId("70"), "hl1-won", "choice wins over the last selected game");
  assert.equal(controller.toolGameId("220"), "hl2-2707", "HL2 resolves to its selected build");
  running = "hl2-4260";
  assert.equal(controller.toolGameId("70"), "hl1-won", "choice wins over the running game");
  assert.equal(controller.toolGameId(""), "hl2-4260", "auto follows the running game");
  assert.equal(controller.toolGameId("999"), "hl2-4260", "unknown choice falls back to auto");
  running = null;
  assert.equal(controller.toolGameId(""), "ghosting-renewed", "auto without a running game uses the last selected one");
});

test("LiveSplit preview compares the prefix with the game by gameId, not by launcher id", { skip: process.platform !== "linux" }, async (t) => {
  const { controller } = fixture(t);
  controller.resolveGameId = (id) => ({ 70: "hl1-won", 218: "ghosting-renewed" })[id] || null;
  controller.tools = { runningInfo: () => ({ gameId: "hl1-won" }) };
  controller.liveSplitOverlay = {
    captureTool: () => ({ snapshot: path.join(os.tmpdir(), "hlsr-capture-test.bin"), status: path.join(os.tmpdir(), "hlsr-capture-test.status") }),
    readSnapshot: () => ({ width: 2, height: 1, pixels: new Uint8Array(8) }),
  };
  controller.protonRunner = {};
  controller.runLinuxLiveSplitCapture = async () => 0;

  const same = await controller.captureLiveSplit("70");
  assert.equal(same.status, "ok");
  assert.equal(same.otherGame, false, "LiveSplit for HL1 is not another game in the HL1 editor");
  const other = await controller.captureLiveSplit("218");
  assert.equal(other.otherGame, true, "LiveSplit for HL1 is another game in the Ghosting editor");
});

test("предпросмотр LiveSplit на Windows: снимается и LiveSplit, запущенный не из лаунчера", { skip: process.platform === "linux" }, async (t) => {
  const { controller } = fixture(t);
  controller.resolveGameId = () => "ghosting-renewed";
  let launched = null;
  controller.tools = { runningInfo: () => launched };
  controller.liveSplitOverlay = {
    captureTool: () => ({ snapshot: path.join(os.tmpdir(), "hlsr-capture-win.bin"), status: path.join(os.tmpdir(), "hlsr-capture-win.status") }),
    readSnapshot: () => ({ width: 2, height: 1, pixels: new Uint8Array(8) }),
  };
  let code = 0;
  controller.liveSplitWindowCapture = { capture: async () => code };

  assert.equal((await controller.captureLiveSplit("218")).status, "ok", "свой LiveSplit пользователя снят");
  code = 2;
  assert.equal((await controller.captureLiveSplit("218")).status, "not-running", "окна нет и лаунчер его не запускал — не запущен");
  launched = { gameId: "ghosting-renewed" };
  assert.equal((await controller.captureLiveSplit("218")).status, "no-window", "запущен из лаунчера, окно свёрнуто");
});

test("файлы оверлея находятся и из исходников (стенд), не только в resources сборки", () => {
  const LiveSplitOverlay = require("../src/main/game-delivery/livesplit-overlay");
  const overlay = new LiveSplitOverlay({ baseDir: os.tmpdir(), platform: "win32" });
  assert.ok(overlay.available(), overlay.resourcesPath);
});

test("CONFIG_CHANGED при записи файла игрока: ответ с ошибкой, бинды в хранилище прежние", (t) => {
  const { store, controller } = fixture(t);
  store.save("ghosting-renewed", { binds: { f: { command: "+duck", base: "" } } });
  const install = require("../src/main/game-delivery/game-binds-install");
  const original = install.applyBinds;
  install.applyBinds = () => { throw Object.assign(new Error("changed"), { code: "CONFIG_CHANGED" }); };
  t.after(() => { install.applyBinds = original; });
  const result = controller.gameSettings("save", "218", { binds: {} });
  assert.equal(result.status, "error");
  assert.equal(result.error.code, "CONFIG_CHANGED");
  assert.deepEqual(Object.keys(store.get("ghosting-renewed").binds), ["f"], "хранилище не тронуто");
});