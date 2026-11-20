"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs"), path = require("path");
const { reactive, computed } = require("vue");
const { parse, babelParse } = require("vue/compiler-sfc");

// Выполняем настоящие методы оболочки с кэшированием computed в Vue.
// Загружать DOM и остальные компоненты для проверки языка не требуется.
function componentScript(component) {
  const file = path.join(__dirname, `../src/renderer/components/Launcher/${component}.vue`);
  const script = parse(fs.readFileSync(file, "utf8")).descriptor.script.content;
  return { script, ast: babelParse(script, { sourceType: "module" }) };
}
const shellScript = componentScript("LauncherShell");
const source = shellScript.script;
const methods = new Map();
function visit(node) {
  if (!node || typeof node !== "object") return;
  if (node.type === "ObjectMethod") methods.set(node.key.name, node);
  for (const value of Object.values(node)) {
    if (Array.isArray(value)) value.forEach(visit);
    else if (value && typeof value === "object") visit(value);
  }
}
visit(shellScript.ast);

// Минимальный экземпляр компонента на реактивности Vue 3: data, computed
// с кэшированием и методы с this, указывающим на экземпляр.
function instance({ data = {}, computed: getters = {}, methods: fns = {} }) {
  const vm = reactive(Object.assign({}, data));
  for (const [name, fn] of Object.entries(getters)) {
    const value = computed(() => fn.call(vm));
    Object.defineProperty(vm, name, { get: () => value.value, configurable: true });
  }
  for (const [name, fn] of Object.entries(fns)) vm[name] = fn.bind(vm);
  return vm;
}
function method(name, settings, ipcRenderer, component = "LauncherShell") {
  let node = methods.get(name);
  let script = source;
  if (component !== "LauncherShell") {
    const parsed = componentScript(component);
    script = parsed.script;
    const find = (value) => {
      if (!value || typeof value !== "object") return;
      if (value.type === "ObjectMethod" && value.key.name === name) node = value;
      for (const child of Object.values(value)) {
        if (Array.isArray(child)) child.forEach(find);
        else if (child && typeof child === "object") find(child);
      }
    };
    find(parsed.ast);
  }
  assert.ok(node, `${component}.${name} exists`);
  const body = script.slice(node.body.start, node.body.end);
  return new Function("settings", "ipcRenderer", "return " + (node.async ? "async " : "") + "function(" + node.params.map(param => script.slice(param.start, param.end)).join(",") + ")" + body)(settings, ipcRenderer);
}

test("код персонализации импортируется только в свою игру, сохраняет остальной черновик и не создаёт изменений при повторе", () => {
  const original = { personalization: { hudEnabled: true, extra: { gh_health_x: 123 } }, packs: { hud: "default" }, bxt: { timer: true } };
  const selected = [];
  const panel = instance({
    data: { gameId: "218", draft: { look: original }, saving: false, currentTheme: "xen", saveError: "", justSaved: "", codeImported: "" },
    methods: { importCodeLook: method("importCodeLook", {}, {}, "GamePersonalization") },
  });
  panel.$refs = { extras: { selectSection: (id) => selected.push(id) } };
  const saved = JSON.stringify(panel.draft.look);
  panel.importCodeLook({ gameId: "70", look: { personalization: { hudEnabled: false } } });
  assert.equal(JSON.stringify(panel.draft.look), saved, "запоздавший ответ другой игры не меняет черновик");
  assert.equal(selected.length, 0);
  panel.importCodeLook({ gameId: "218", look: { personalization: JSON.parse(saved).personalization } });
  assert.equal(JSON.stringify(panel.draft.look), saved, "одинаковый код не создаёт лишних изменений");
  panel.importCodeLook({ gameId: "218", look: { personalization: { hudEnabled: true, extra: { gh_health_x: 456 } } } });
  assert.equal(panel.draft.look.personalization.extra.gh_health_x, 456);
  assert.deepEqual(JSON.parse(JSON.stringify(panel.draft.look.packs)), original.packs);
  assert.deepEqual(JSON.parse(JSON.stringify(panel.draft.look.bxt)), original.bxt);
  assert.equal(panel.currentTheme, "xen");
  assert.deepEqual(selected, ["personalization", "personalization"]);
  panel.saving = true;
  panel.importCodeLook({ gameId: "218", look: { personalization: { hudEnabled: false } } });
  assert.equal(panel.draft.look.personalization.extra.gh_health_x, 456, "импорт не перебивает сохранение");
});

test("interface language can switch repeatedly in both directions without resetting settings", () => {
  const data = { config: { language: 1, gpuAcceleration: false } };
  const settings = { get: key => data[key], set: (key, value) => { data[key] = value; return true; } };
  const localization = { lang: 1, locales: { en: {}, ru: {} }, update() { this.lang = data.config.language; } };
  const synchronized = [];
  const shell = instance({
    data: { langRev: 0 },
    computed: { currentLanguage: method("currentLanguage", settings) },
    methods: { setLanguage: method("setLanguage", settings), onLanguageChange: method("onLanguageChange", settings), syncUpdaterLanguage() { synchronized.push(this.currentLanguage); } },
  });
  shell.$localization = localization;
  assert.equal(shell.currentLanguage, "ru");
  for (const language of ["en", "ru", "en", "ru"]) {
    shell.setLanguage(language);
    assert.equal(shell.currentLanguage, language);
    assert.equal(localization.lang, language === "ru" ? 1 : 0);
  }
  shell.setLanguage("ru"); shell.setLanguage("unknown");
  assert.equal(shell.langRev, 4);
  assert.deepEqual(synchronized, ["en", "ru", "en", "ru"]);
  assert.equal(data.config.gpuAcceleration, false);
});

function notificationCenter(t, ipc = { invoke: async () => ({ items: [], unread: 0 }) }) {
  const center = method("data", undefined, ipc, "NotificationCenter")();
  center.$refs = {};
  center.$nextTick = (fn) => fn();
  center.suspended = false;
  for (const name of ["refresh", "applySnapshot", "nextPreview", "pausePreview", "resumePreview", "dismissPreview", "afterPreviewLeave", "isUpdateAvailable"])
    center[name] = method(name, undefined, ipc, "NotificationCenter").bind(center);
  t.after(() => { center.disposed = true; clearTimeout(center.previewTimer); });
  return center;
}

test("saved first-install notifications pop out of the bell once and remain unread after dismissal", (t) => {
  const center = notificationCenter(t);
  const start = { id: "start", code: "PROTON_INSTALLING", read: false, ts: Date.now() };
  center.applySnapshot({ items: [start], unread: 1 });
  assert.equal(center.preview.id, start.id);
  center.dismissPreview();
  assert.equal(center.preview, null);
  assert.equal(center.unread, 1);
  assert.equal(center.items[0].read, false);
  center.applySnapshot({ items: [start], unread: 1 });
  assert.equal(center.preview, null);
});

test("Proton notification button starts the explicit update and opens its progress in settings", () => {
  const calls = [];
  const shell = { settingsOpen: false, settingsSection: "general" };
  const onUpdate = method("onRunUpdateAction", undefined, {
    invoke: (channel) => { calls.push(channel); return Promise.resolve(); },
  });
  onUpdate.call(shell, { target: "proton" });
  assert.deepEqual(calls, ["proton-update"]);
  assert.equal(shell.settingsOpen, true);
  assert.equal(shell.settingsSection, "proton");
});

test("an unread Proton update offer pops up from history at startup without downloading", (t) => {
  const calls = [];
  const center = notificationCenter(t, { invoke: async (...args) => { calls.push(args); } });
  const old = { id: "old", code: "LAUNCH_FAILED", read: false, ts: 1 };
  const offer = { id: "offer", code: "PROTON_UPDATE_AVAILABLE", read: false, ts: 1 };
  center.applySnapshot({ items: [offer, old], unread: 2 });
  assert.equal(center.preview.id, offer.id);
  assert.deepEqual(center.previewQueue, []);
  center.applySnapshot({ items: [offer, old], unread: 2 });
  assert.deepEqual(center.previewQueue, []);
  assert.deepEqual(calls, []);
});

test("saved notification previews are queued through modals and advances after returning to the bell", (t) => {
  const center = notificationCenter(t);
  center.suspended = true;
  const first = { id: "first", code: "PROFILE_IMPORTED", read: false, ts: Date.now() };
  const second = { id: "second", code: "GAME_INSTALLED", read: false, ts: Date.now() };
  center.applySnapshot({ items: [second, first], unread: 2 });
  assert.equal(center.preview, null);
  assert.deepEqual(center.previewQueue, [first, second]);
  center.suspended = false;
  center.nextPreview();
  assert.equal(center.preview.id, first.id);
  center.$refs.preview = { isConnected: true };
  center.dismissPreview();
  assert.equal(center.returning, true);
  assert.equal(center.preview, null);
  center.$refs.preview = null;
  center.afterPreviewLeave();
  assert.equal(center.preview.id, second.id);
});

test("opening the notification list prevents new notifications from also opening a preview", (t) => {
  const center = notificationCenter(t);
  center.applySnapshot({ items: [], unread: 0 });
  center.open = true;
  const item = { id: "new", code: "PROFILE_IMPORTED", read: false, ts: Date.now() };
  center.applySnapshot({ items: [item], unread: 1 });
  assert.equal(center.preview, null);
  assert.deepEqual(center.previewQueue, []);
  assert.equal(center.items[0].id, item.id);
});

test("coalescing a notification updates its preview and read notifications are removed from the queue", (t) => {
  const center = notificationCenter(t);
  const item = { id: "new", code: "LAUNCH_FAILED", read: false, count: 1, ts: Date.now() };
  center.applySnapshot({ items: [item], unread: 1 });
  center.applySnapshot({ items: [{ ...item, count: 2 }], unread: 1 });
  assert.equal(center.preview.count, 2);
  assert.deepEqual(center.previewQueue, []);
  center.applySnapshot({ items: [{ ...item, count: 2, read: true }], unread: 0 });
  assert.equal(center.preview, null);
  assert.deepEqual(center.previewQueue, []);
});

test("stale IPC responses cannot overwrite the latest notification snapshot", async (t) => {
  const pending = [];
  const center = notificationCenter(t, { invoke: () => new Promise(resolve => pending.push(resolve)) });
  const first = center.refresh();
  const second = center.refresh();
  const item = { id: "fresh", code: "PROFILE_IMPORTED", read: false, ts: Date.now() };
  pending[1]({ items: [item], unread: 1 });
  await second;
  pending[0]({ items: [], unread: 0 });
  await first;
  assert.equal(center.items[0].id, item.id);
  assert.equal(center.preview.id, item.id);
});

test("sidebar click hides the tooltip before emitting an action, mouse focus restoration does not reopen it", () => {
  const sidebar = { tip: { title: "Settings" }, hideTip() { this.tip = null; }, $emit(event, id) { assert.equal(this.tip, null); this.action = [event, id]; }, showTip() { this.tip = { title: "Settings" }; } };
  method("selectItem", undefined, undefined, "GameSidebar").call(sidebar, "tool", "settings");
  assert.deepEqual(sidebar.action, ["tool", "settings"]);
  method("showFocusTip", undefined, undefined, "GameSidebar").call(sidebar, { currentTarget: { matches: () => false } }, {});
  assert.equal(sidebar.tip, null);
  method("showFocusTip", undefined, undefined, "GameSidebar").call(sidebar, { currentTarget: { matches: () => true } }, {});
  assert.ok(sidebar.tip);
});

test("ghost deletion requires inline confirmation and removes only the confirmed row", async () => {
  const calls = [];
  const ipc = { invoke: async (...args) => { calls.push(args); return { status: "ok" }; } };
  const run = { id: "one", name: "one.run" }, other = { id: "two", name: "two.run" };
  const modal = { gameId: "ghosting-renewed", runs: [run, other], pendingRunDelete: null, deletingRun: null, runsTruncated: false };
  const remove = method("deleteRun", undefined, ipc, "GameSettingsModal");
  await remove.call(modal, run);
  assert.deepEqual(calls, []);
  method("confirmRunDelete", undefined, ipc, "GameSettingsModal").call(modal, run);
  await remove.call(modal, run);
  assert.deepEqual(calls, [["game-runs-delete", "ghosting-renewed", "one.run"]]);
  assert.deepEqual(modal.runs, [other]);
  assert.equal(modal.pendingRunDelete, null);
  assert.equal(modal.deletingRun, null);
});

test("failed ghost deletion keeps the row and confirmation available for retry", async () => {
  const run = { id: "one", name: "one.run" };
  const modal = { gameId: "ghosting-renewed", runs: [run], pendingRunDelete: run.id, deletingRun: null, $t: (key) => key };
  await method("deleteRun", undefined, { invoke: async () => ({ status: "error", code: "GAME_RUNNING" }) }, "GameSettingsModal").call(modal, run);
  assert.deepEqual(modal.runs, [run]);
  assert.equal(modal.pendingRunDelete, run.id);
  assert.equal(modal.deletingRun, null);
  assert.equal(modal.runDeleteError, "#L_D_ERR_GAME_RUNNING");
});

test("game settings reach main as a plain copy, not as Vue reactive proxies", async (t) => {
  // submit() берёт EMPTY и defaultPersonalization из модуля компонента.
  globalThis.EMPTY = { rinput: true, personalization: null, packs: null, binds: {} };
  globalThis.defaultPersonalization = () => ({ extra: {} });
  globalThis.withPersonalization = (settings) => settings;
  t.after(() => { delete globalThis.EMPTY; delete globalThis.defaultPersonalization; delete globalThis.withPersonalization; });
  const sent = [];
  // Electron клонирует аргументы IPC так же, как structuredClone, и прокси не пропускает.
  const ipc = { invoke: async (channel, ...args) => { sent.push(structuredClone([channel, ...args])); return { status: "ok", settings: {} }; } };
  const modal = instance({ data: { gameId: "ghosting-renewed", saving: false, modSettingsInvalid: false, argsError: null,
    configBuild: "ghosting-renewed", configChanges: new Proxy({ sensitivity: 2.5 }, {}), configError: null,
    draft: { rinput: false, personalization: { extra: { gh_hud_bg: true } }, packs: { a: true }, binds: { F5: { command: "save", base: "" } } } } });
  modal.$emit = (event) => { modal.emitted = (modal.emitted || []).concat(event); };
  modal.loadConfig = async () => {};
  modal.loadRuleCheck = () => {};
  modal.saveConfig = method("saveConfig", undefined, ipc, "GameSettingsModal");
  await method("submit", undefined, ipc, "GameSettingsModal").call(modal);
  assert.deepEqual(sent, [
    ["game-config-write", "ghosting-renewed", "ghosting-renewed", { sensitivity: 2.5 }],
    ["game-settings-save", "ghosting-renewed", { rinput: false, personalization: { extra: { gh_hud_bg: true } },
      packs: { a: true }, binds: { F5: { command: "save", base: "" } } }],
  ]);
  assert.deepEqual(modal.emitted, ["saved"], "окно остаётся открытым, тост даёт saved");
  assert.equal(modal.saving, false);
});

test("правленые блоки скриптов: сохранение спрашивает об удалении и повторяется с removeEdited", async (t) => {
  globalThis.EMPTY = { binds: {} };
  globalThis.defaultPersonalization = () => ({ extra: {} });
  globalThis.withPersonalization = (settings) => settings;
  t.after(() => { delete globalThis.EMPTY; delete globalThis.defaultPersonalization; delete globalThis.withPersonalization; });
  const sent = [];
  let kept = ["+fps_20_hold"];
  const ipc = { invoke: async (channel, ...args) => { sent.push([channel, ...args]); return { status: "ok", settings: {}, keptEdited: kept }; } };
  const modal = instance({ data: { gameId: "hl1-won", saving: false, argsError: null, configChanges: {}, editedPending: null, draft: { binds: {} } } });
  modal.$emit = () => {};
  modal.saveConfig = async () => true;
  modal.loadRuleCheck = () => {};
  modal.submit = method("submit", undefined, ipc, "GameSettingsModal");
  await modal.submit();
  assert.deepEqual(modal.editedPending, ["+fps_20_hold"], "ждёт ответа игрока");
  assert.equal(sent[0].length, 3, "без removeEdited канал получает прежние аргументы");

  kept = [];
  await method("deleteEdited", undefined, ipc, "GameSettingsModal").call(modal);
  assert.deepEqual(sent[1].slice(3), [undefined, { removeEdited: true }]);
  assert.equal(modal.editedPending, null);

  const template = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameSettingsModal.vue"), "utf8");
  for (const key of ["#L_GS_SCRIPT_EDITED_TITLE", "#L_GS_SCRIPT_EDITED_TEXT", "#L_GS_SCRIPT_EDITED_DELETE"]) {
    assert.ok(template.includes(key), `${key} в окне`);
    for (const lang of ["ru", "en"]) {
      assert.ok(fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8").includes(`"${key}"`), `${key} ${lang}`);
    }
  }
});

test("CONFIG_CHANGED при сохранении: окно показывает «Файл изменён вне лаунчера» и не говорит «сохранено»", async (t) => {
  globalThis.EMPTY = { binds: {} };
  globalThis.defaultPersonalization = () => ({ extra: {} });
  t.after(() => { delete globalThis.EMPTY; delete globalThis.defaultPersonalization; });
  const ipc = { invoke: async () => ({ status: "error", error: { code: "CONFIG_CHANGED" } }) };
  const emitted = [];
  const modal = instance({ data: { gameId: "hl1-won", saving: false, argsError: null, configChanges: {}, fileChanged: false, editedPending: null, draft: { binds: {} } } });
  modal.$emit = (event) => emitted.push(event);
  modal.saveConfig = async () => true;
  await method("submit", undefined, ipc, "GameSettingsModal").call(modal);
  assert.equal(modal.fileChanged, true);
  assert.deepEqual(emitted, []);
  for (const key of ["#L_GS_CFG_CHANGED", "#L_GS_CFG_CHANGED_RELOAD"]) {
    for (const lang of ["ru", "en"]) {
      assert.ok(fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8").includes(`"${key}"`), `${key} ${lang}`);
    }
  }
});

test("«Конфиги»: запись уходит с отпечатком файла, CONFIG_CHANGED показывает «Перечитать», фокус перечитывает без правок", async () => {
  const sent = [];
  let reply = { status: "ok", sha256: "new" };
  const ipc = { invoke: async (...args) => { sent.push(args); return reply; } };
  const modal = instance({ data: { gameId: "hl1-won", configBuild: "hl1-won", configChanges: { fps_max: 60 }, configError: null,
    configSha256: "old", configState: { status: "ok", sha256: "abc" }, fileChanged: false, saving: false, section: "launch" } });
  modal.configErrorText = () => "err";
  const save = method("saveConfig", undefined, ipc, "GameSettingsModal");
  assert.equal(await save.call(modal), true);
  assert.deepEqual(sent[0], ["game-config-write", "hl1-won", "hl1-won", { fps_max: 60 }, { expectedSha256: "abc" }]);
  assert.equal(modal.configSha256, "new", "отпечаток после записи уходит в остальные настройки окна");
  assert.equal(modal.configState.sha256, "new", "повторная отправка после сбоя дальнейшего сохранения несёт новый отпечаток");
  await save.call(modal);
  assert.deepEqual(sent[1].slice(4), [{ expectedSha256: "new" }]);
  reply = { status: "ok", sha256: "new" };
  modal.configState.sha256 = "abc";

  reply = { status: "error", code: "CONFIG_CHANGED" };
  assert.equal(await save.call(modal), false);
  assert.equal(modal.fileChanged, true);
  assert.equal(modal.configError, null, "второго окна ошибки нет");
  assert.equal(modal.section, "configs");

  let reads = 0, loads = 0;
  const focus = method("onWindowFocus", undefined, ipc, "GameSettingsModal");
  const unsaved = method("hasUnsaved", undefined, ipc, "GameSettingsModal");
  const idle = { saving: false, fileChanged: false, configChanges: {}, pendingCfgImport: null, draft: { a: 1 }, draftSaved: '{"a":1}',
    load: async () => { loads += 1; return true; }, loadConfig: async () => { reads += 1; } };
  idle.hasUnsaved = unsaved;
  await focus.call(idle);
  await focus.call({ ...idle, configChanges: { fps_max: 60 } });
  await focus.call({ ...idle, draft: { a: 2 } });
  await focus.call({ ...idle, pendingCfgImport: {} });
  await focus.call({ ...idle, saving: true });
  assert.deepEqual([loads, reads], [1, 1], "без правок перечитывается всё окно, с правками — ничего");

  // Гонка: ответ перечитывания приходит после правки или начала сохранения — отбрасывается.
  const race = async (mutate) => {
    let answer;
    let calls = 0;
    // Первый ответ (game-settings-get) задерживается, следующие приходят сразу.
    const ipcSlow = { invoke: () => (calls++ ? Promise.resolve({ status: "ok", sha256: "disk" }) : new Promise((resolve) => { answer = resolve; })) };
    const vm = instance({ data: { gameId: "hl1-won", configBuild: "hl1-won", saving: false, fileChanged: false, configChanges: {}, pendingCfgImport: null,
      draft: { a: 1 }, draftSaved: '{"a":1}', configSha256: "old", languages: [], keyBinds: null, bindsGameId: null, configState: { sha256: "old" } } });
    globalThis.withPersonalization = (value) => value;
    vm.hasUnsaved = unsaved;
    vm.load = method("load", undefined, ipcSlow, "GameSettingsModal");
    vm.loadConfig = method("loadConfig", undefined, ipcSlow, "GameSettingsModal");
    vm.loadRuleCheck = () => {};
    const run = method("onWindowFocus", undefined, ipcSlow, "GameSettingsModal").call(vm);
    mutate(vm);
    answer({ status: "ok", settings: { a: 9 }, configSha256: "disk" });
    await run;
    delete globalThis.withPersonalization;
    return vm;
  };
  let vm = await race((m) => { m.draft.a = 2; });
  assert.deepEqual([vm.draft.a, vm.configSha256], [2, "old"], "правка во время чтения не затёрта");
  vm = await race((m) => { m.saving = true; m.focusToken = (m.focusToken || 0) + 1; });
  assert.deepEqual([vm.draft.a, vm.configSha256], [1, "old"], "ответ после начала сохранения отброшен");
  vm = await race(() => {});
  assert.deepEqual([vm.draft.a, vm.configSha256], [9, "disk"], "без правок ответ применяется");

  const slow = [];
  const pzIpc = { invoke: () => new Promise((resolve) => slow.push(resolve)) };
  const pzRace = { visible: true, saving: false, conflictGameId: "", gameId: "hl1-won", drafts: { "hl1-won": { look: 1, saved: "1", configSha256: "old" } },
    isDirty() { return this.drafts["hl1-won"].look !== 1; } };
  pzRace.load = method("load", undefined, pzIpc, "GamePersonalization");
  const pending = method("onWindowFocus", undefined, pzIpc, "GamePersonalization").call(pzRace);
  pzRace.drafts["hl1-won"].look = 5;
  slow[0]({ status: "ok", settings: {}, configSha256: "disk" });
  await pending;
  assert.deepEqual([pzRace.drafts["hl1-won"].look, pzRace.drafts["hl1-won"].configSha256], [5, "old"], "панель: правка не затёрта");

  const panel = method("onWindowFocus", undefined, ipc, "GamePersonalization");
  const loaded = [];
  const pz = { visible: true, saving: false, gameId: "hl1-won", drafts: { "hl1-won": { look: 1, saved: "1" } }, load: (id) => loaded.push(id), isDirty: () => false };
  panel.call(pz);
  panel.call({ ...pz, isDirty: () => true });
  panel.call({ ...pz, visible: false });
  assert.deepEqual(loaded, ["hl1-won"], "панель перечитывает активную игру без черновика");
  const pzSource = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GamePersonalization.vue"), "utf8");
  assert.ok(pzSource.includes('addEventListener("focus", this.onWindowFocus)') && pzSource.includes('removeEventListener("focus", this.onWindowFocus)'));

  const editor = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameConfigEditor.vue"), "utf8");
  assert.ok(editor.includes("this.playerFile.toLowerCase()"), "чип не показывается для файла игрока");

  const view = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameSettingsModal.vue"), "utf8");
  assert.ok(view.includes('addEventListener("focus", this.onWindowFocus)') && view.includes('removeEventListener("focus", this.onWindowFocus)'));
  assert.ok(fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameConfigEditor.vue"), "utf8").includes("#L_GS_CFG_SHADOWED"));
  for (const lang of ["ru", "en"]) {
    assert.ok(fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8").includes('"#L_GS_CFG_SHADOWED"'), lang);
  }
});

test("R18: вкладки перечитывают файл при возврате, редактор cfg сообщает о правке и сохранении, итог импорта снимается", async () => {
  let focused = 0;
  const pz = { visible: false, measureInk() {}, $nextTick: (fn) => fn(), onWindowFocus() { focused += 1; } };
  method("activated", undefined, {}, "GamePersonalization").call(pz);
  assert.deepEqual([pz.visible, focused], [true, 1], "«Персонализация» перечитывает файл при возврате на вкладку");

  let gsFocused = 0;
  method("activated", undefined, {}, "GameConfigsPanel").call({ settings: () => ({ onWindowFocus() { gsFocused += 1; } }) });
  method("activated", undefined, {}, "GameConfigsPanel").call({ settings: () => null });
  assert.equal(gsFocused, 1, "«Конфиги» перечитывают окно при возврате на вкладку");

  const unsaved = method("hasUnsaved", undefined, {}, "GameSettingsModal");
  const idle = { draftSaved: "{}", draft: {}, configChanges: {}, pendingCfgImport: null, cfgFilesDirty: false };
  assert.equal(unsaved.call(idle), false);
  assert.equal(unsaved.call({ ...idle, cfgFilesDirty: true }), true, "несохранённый текст cfg — несохранённое окно");

  const events = [];
  let reloads = 0;
  const modal = { cfgFilesDirty: true, $emit: (event) => events.push(event), loadRuleCheck() {}, onWindowFocus: async () => { reloads += 1; } };
  await method("onCfgFileSaved", undefined, {}, "GameSettingsModal").call(modal);
  assert.deepEqual([modal.cfgFilesDirty, events, reloads], [false, ["file-saved"], 1], "после сохранения текста окно перечитывает файл");

  const saved = [];
  const files = { dirty: true, saving: false, text: "x", current: { path: "cfg/autoexec.cfg", content: "", encoding: "utf8", eol: "\r\n", mtime: 1 },
    gameId: "g", build: null, notice: null, running: false, baseName: (file) => file.split("/").pop(), $t: (key) => key,
    $emit: (event, value) => saved.push([event, value]), errorText: () => "err" };
  await method("save", undefined, { invoke: async () => ({ status: "ok", mtime: 2 }) }, "GameCfgFiles").call(files);
  assert.deepEqual(saved, [["saved", { path: "cfg/autoexec.cfg" }]]);

  const emitted = [];
  const cfgp = { notice: { undo: true }, $emit: (event) => emitted.push(event) };
  method("onSaved", undefined, {}, "GameConfigsPanel").call(cfgp, {});
  assert.deepEqual([cfgp.notice, emitted], [null, ["saved"]], "сохранение снимает «Вернуть прежний»");

  const read = (name) => fs.readFileSync(path.join(__dirname, `../src/renderer/components/Launcher/${name}.vue`), "utf8");
  const gsm = read("GameSettingsModal");
  assert.match(gsm, /<button\s+v-if="!embedded"\s+type="button"\s+class="settings-btn ghost"\s+:aria-busy="importingBinds/, "импорт окна клавиатуры скрыт во вкладке (I3)");
  assert.equal((gsm.match(/@saved="onCfgFileSaved"/g) || []).length, 2);
  assert.equal((gsm.match(/@dirty="cfgFilesDirty = \$event"/g) || []).length, 2);
  assert.match(read("GameCfgFiles"), /beforeUnmount\(\) \{[^}]*if \(this\.dirty\) this\.\$emit\("dirty", false\)/, "закрытый раздел снимает флаг несохранённого (N2)");
  const panel = read("GameConfigsPanel");
  assert.ok(panel.includes('@saved="onSaved"') && panel.includes('@file-saved="notice = null"'));
});

test("M5: «Сбросить» в «Персонализации» — черновик есть что сохранить, сохранение уходит с reset", async () => {
  globalThis.resetExtras = (extra) => extra || {};
  globalThis.defaultPersonalization = () => ({ extra: {} });
  globalThis.defaultOverlay = () => ({});
  globalThis.snapshot = (look) => JSON.stringify(look);
  globalThis.pickLook = (settings) => settings;
  try {
    const entry = { settings: {}, look: { personalization: { extra: {} }, liveSplitOverlay: {} }, configSha256: "sha" };
    entry.saved = JSON.stringify(entry.look);
    const pz = { gameId: "hl1-won", drafts: { "hl1-won": entry }, isGoldSrc: false, codeImported: "", saving: false, invalid: {}, saveError: "", conflictGameId: "",
      get draft() { return this.drafts[this.gameId]; }, $t: (key) => key };
    pz.isDirty = method("isDirty", undefined, {}, "GamePersonalization").bind(pz);
    method("resetGame", undefined, {}, "GamePersonalization").call(pz);
    assert.equal(pz.isDirty("hl1-won"), true, "сброс к уже показанным умолчаниям всё равно ждёт «Сохранить»");
    const sent = [];
    await method("save", undefined, { invoke: async (...args) => { sent.push(args); return { status: "ok", settings: {}, configSha256: "new" }; } }, "GamePersonalization").call(pz);
    assert.deepEqual(sent[0][4], { configSha256: "sha", reset: true });
    assert.equal(pz.drafts["hl1-won"].reset, undefined, "после сохранения флага нет");
    method("revertGame", undefined, {}, "GamePersonalization").call(Object.assign(pz, { drafts: { "hl1-won": { ...entry, reset: true } } }));
    assert.equal(pz.drafts["hl1-won"].reset, false);
  } finally {
    for (const name of ["resetExtras", "defaultPersonalization", "defaultOverlay", "snapshot", "pickLook"]) delete globalThis[name];
  }
});

test("M4, M7, M9: тексты вкладки «Конфиги» на «ты» и по фактам, «Конфиги» при запущенной игре, фокус опроса на «Пропустить»", () => {
  const read = (file) => fs.readFileSync(path.join(__dirname, "../src/renderer", file), "utf8");
  const ru = read("utils/locales/ru.locale.js");
  const en = read("utils/locales/en.locale.js");
  const value = (locale, key) => JSON.parse(new RegExp(`"${key}": ("(?:[^"\\\\]|\\\\.)*")`).exec(locale)[1]);
  const keys = ["#L_CF_TRUNCATED", "#L_CF_PICK", "#L_CF_RUNNING_CONFIG", "#L_CF_SAVED_RUNNING", "#L_CF_ERR_NOT_FOUND", "#L_CF_ERR_CHANGED",
    "#L_CF_ERR_ENCODING", "#L_SCR_OWN_ALIAS", "#L_SCR_PRESS_KEY", "#L_SCR_SENSITIVITY", "#L_KB_SAVED_RUNNING", "#L_KB_CLICK_HINT", "#L_KB_PICK_KEY",
    "#L_GS_CFG_RUNNING", "#L_GS_GHOST_DELETE_CONFIRM", "#L_GS_ERR_WRAPPER", "#L_KB_IMPORT_CFG_HINT", "#L_KB_IMPORT_REMOVE_HINT", "#L_KB_IMPORT_REMOVED"];
  for (const key of keys) assert.doesNotMatch(value(ru, key), /[а-яё]+(?:ите|йте|ьте)\b|(?:^|[^а-яё])(?:[Вв]ы|[Вв]аш[а-яё]*|[Вв]ам)(?![а-яё])/i, key);
  // Импорт заменяет файл игрока и кладёт прежний в копию; exec-строк и «отключения» больше нет.
  for (const key of ["#L_KB_IMPORT_CFG_HINT", "#L_KB_IMPORT_REMOVE_HINT", "#L_KB_IMPORT_REMOVED"]) {
    assert.doesNotMatch(value(ru, key), /userconfig\.cfg|строки exec|отключ|главнее/, key);
    assert.doesNotMatch(value(en, key), /userconfig\.cfg|exec lines|disable|still win/i, key);
  }
  assert.match(value(ru, "#L_GS_CFG_RUNNING"), /%0%/);
  assert.match(value(en, "#L_GS_CFG_RUNNING"), /%0%/);

  const modal = read("components/Launcher/GameSettingsModal.vue");
  const editorTag = modal.slice(modal.indexOf("<GameConfigEditor"), modal.indexOf(">", modal.indexOf("<GameConfigEditor")));
  assert.ok(editorTag.includes(':value="configChanges"') && !editorTag.includes(":disabled"), "редактор «Конфигов» не блокируется запущенной игрой");
  assert.ok(modal.includes('$t("#L_GS_CFG_RUNNING", configState.playerFile)'));
  const index = fs.readFileSync(path.join(__dirname, "../src/main/game-delivery/index.js"), "utf8");
  const write = index.slice(index.indexOf("  writeGameConfig(id, build, changes, options) {"), index.indexOf("  configInfo(id, build) {"));
  assert.ok(!write.includes("GAME_RUNNING"));

  const setup = read("components/Launcher/ConfigSetupModal.vue");
  assert.ok(setup.includes(`querySelector('.cfgsetup__actions [data-choice="skip"]')`), "Enter после «Играть» выбирает «Пропустить»");
});

test("I1: Half-Life 2 один чип; M1: подвал только там, где есть что сохранять; M5: удаление демо без учёта регистра; M10: сброс не трогает тему", () => {
  const { configChips } = require("../src/renderer/components/Launcher/config-chips");
  const hl1 = { id: "70", name: "HL1" }, hl2 = { id: "220", name: "HL2" };
  const withHl2 = configChips([hl1, hl2]);
  assert.equal(withHl2.filter((chip) => /Half-Life 2/.test(chip.name)).length, 1, "один чип HL2");
  const installed = withHl2.find((chip) => chip.id === "220");
  assert.deepEqual([installed.name, installed.soon, installed.disabled], ["Half-Life 2", true, false], "установленный HL2 выбирается и помечен «в разработке»");
  const without = configChips([hl1]);
  const soon = without.find((chip) => chip.name === "Half-Life 2");
  assert.deepEqual([without.length, soon.soon, soon.disabled], [3, true, true], "без HL2 — неактивный чип");
  // LiveSplit — последним, всегда неактивный и «в разработке», как неустановленный HL2.
  for (const chips of [withHl2, without])
    assert.deepEqual(chips[chips.length - 1], { id: "", name: "LiveSplit", soon: true, disabled: true }, "LiveSplit после Half-Life 2");
  assert.deepEqual(without.map((chip) => chip.name), ["HL1", "Half-Life 2", "LiveSplit"]);
  assert.equal(new Set(without.map((chip) => chip.id || chip.name)).size, without.length, "ключи чипов разные");

  const read = (name) => fs.readFileSync(path.join(__dirname, `../src/renderer/components/Launcher/${name}.vue`), "utf8");
  const modal = read("GameSettingsModal");
  assert.ok(modal.includes('["demos", "ghosts", "version"].includes(this.activeSection)'), "подвал скрыт на демо, призраках и версии");
  assert.ok(modal.includes(':disabled="saving || !hasUnsaved()"'), "«Сохранить» только при правках");
  assert.ok(read("GameDemos").includes("item.name.toLowerCase() === name"), "isUnique без учёта регистра");
  const settings = read("SettingsModal");
  assert.ok(/resetDraft\(\) \{[\s\S]*?launcherTheme: this\.prefs\.launcherTheme/.test(settings), "сброс держит применённую тему");
});

test("dropdown typeahead jumps by first letters and cycles on a repeated letter", async () => {
  const { findTypeaheadIndex, isTypeaheadKey } = await import(
    require("url").pathToFileURL(path.join(__dirname, "../src/renderer/components/Launcher/dropdown-typeahead.mjs")).href
  );
  const items = ["Не указана", "Германия", "Россия", "Румыния", "Украина", "Ёлка"].map((label, id) => ({ id, label }));
  assert.equal(findTypeaheadIndex(items, "р", -1), 2);
  assert.equal(findTypeaheadIndex(items, "р", 2), 3, "та же буква ведёт к следующей стране");
  assert.equal(findTypeaheadIndex(items, "рр", 3), 2, "перебор идёт по кругу");
  assert.equal(findTypeaheadIndex(items, "ру", 2), 3, "набранный префикс уточняет выбор");
  assert.equal(findTypeaheadIndex(items, "РОС", -1), 2);
  assert.equal(findTypeaheadIndex(items, "е", -1), 5, "ё ищется и по е");
  assert.equal(findTypeaheadIndex(items, "щ", 0), -1);
  assert.equal(isTypeaheadKey({ key: "р" }), true);
  assert.equal(isTypeaheadKey({ key: "a", ctrlKey: true }), false);
  assert.equal(isTypeaheadKey({ key: "Enter" }), false);
  assert.equal(isTypeaheadKey({ key: " " }), false);
});

test("profile links accept a channel name or link and keep only the right service", () => {
  const links = require("../src/shared/profile-links");
  assert.equal(links.normalize("youtube", "@kanoyo"), "https://www.youtube.com/@kanoyo");
  assert.equal(links.normalize("youtube", "kanoyo"), "https://www.youtube.com/@kanoyo");
  assert.equal(links.normalize("youtube", "youtube.com/@kanoyo/videos"), "https://www.youtube.com/@kanoyo", "https:// не обязателен");
  assert.equal(links.normalize("youtube", "http://m.youtube.com/@kanoyo"), "https://www.youtube.com/@kanoyo");
  assert.equal(links.normalize("youtube", "youtube.com/channel/UC1234567890123456789012"), "https://www.youtube.com/channel/UC1234567890123456789012");
  assert.equal(links.normalize("youtube", "https://www.twitch.tv/kanoyo"), null, "ссылка не на YouTube");
  assert.equal(links.normalize("youtube", "youtube.com/watch?v=abc"), null, "видео — не канал");
  assert.equal(links.normalize("youtube", "https://youtube.com@evil.example/@x"), null);
  assert.equal(links.normalize("youtube", ""), "", "пустое поле — без ссылки");
  assert.equal(links.normalize("twitch", "Kanoyo"), "https://www.twitch.tv/Kanoyo");
  assert.equal(links.normalize("twitch", "twitch.tv/kanoyo/videos"), "https://www.twitch.tv/kanoyo");
  assert.equal(links.normalize("twitch", "twitch.tv/directory"), null, "служебная страница — не канал");
  assert.equal(links.normalize("twitch", "kick.com/kanoyo"), null);
  assert.equal(links.channelName("youtube", "https://www.youtube.com/@kanoyo"), "@kanoyo");
  assert.equal(links.channelName("youtube", "https://www.youtube.com/channel/UC1234567890123456789012"), null);
  assert.equal(links.channelName("twitch", "https://www.twitch.tv/Kanoyo"), "Kanoyo");
});

test("nick style keeps only presets and safe text CSS", () => {
  const nick = require("../src/shared/nick-styles");
  const state = require("../src/main/online-profile/profile-state");
  assert.equal(nick.sanitizeCss(" COLOR : #ff0 ;text-shadow: 0 0 8px rgba(255, 255, 0, .5); "), "color: #ff0; text-shadow: 0 0 8px rgba(255, 255, 0, .5)");
  for (const css of ["background: url(https://evil.example/x.png)", "color: red; } body { display: none", "width: 9999px",
    "position: fixed", "font-size: 200px", "content: \"x\"", "@import x", "color: \\72 ed", "color: rgb(1, 2", "color red"]) {
    assert.equal(nick.sanitizeCss(css), null, css);
  }
  assert.deepEqual(nick.normalize({ style: "developer", css: "" }), { style: "developer", css: "" });
  assert.equal(nick.normalize({ style: "retired", css: "width: 1px" }), null, "неизвестный стиль и чужой CSS отброшены");
  const profile = state.normalize({ steamId64: "76561198000000000", personaName: "x", games: {}, details: {}, nick: { style: "gold" } });
  assert.deepEqual(state.state(profile, {}, true).nick, { style: "gold", css: "" });
});

test("0.4.9: лишние тексты убраны", () => {
  const root = path.join(__dirname, "../src/renderer");
  const read = (p) => fs.readFileSync(path.join(root, p), "utf8");
  const locales = ["utils/locales/ru.locale.js", "utils/locales/en.locale.js", "components/Launcher/runner-locales.js"].map(read).join("\n");
  for (const key of ["#L_RUNNER_TIME_NOTE", "#L_S_DISCORD_RPC_NOTE", "#L_GM_UPDATES"]) assert.ok(!locales.includes(`"${key}"`), key);
  assert.ok(!read("components/Launcher/GameMenu.vue").includes('id: "updates"'));
  assert.ok(!read("components/Launcher/LauncherShell.vue").includes('case "updates"'));
  const account = read("components/Launcher/AccountMenu.vue");
  const short = account.slice(account.indexOf("statusShort()"));
  assert.ok(!short.slice(0, short.indexOf("default:")).includes("#L_RUNNER_ONLINE"), "statusShort не выводит «синхронизирован»");
});

// Значок подписчика у ника: каждый ник в интерфейсе знает о подписке,
// картинка не попадает под оформление ника, свечение не «дышит» без анимаций.
test("значок подписчика: у каждого NickName есть :subscriber, значок вне .nick, без анимаций стоит", () => {
  const launcher = path.join(__dirname, "../src/renderer/components/Launcher");
  const read = (file) => fs.readFileSync(file, "utf8");
  const tags = [];
  for (const name of fs.readdirSync(launcher).filter((file) => file.endsWith(".vue") && file !== "NickName.vue")) {
    const template = parse(read(path.join(launcher, name))).descriptor.template;
    for (const match of (template ? template.content : "").matchAll(/<NickName\b[^>]*>/g)) tags.push([name, match[0]]);
  }
  assert.ok(tags.length >= 9, "ники найдены: " + tags.length);
  for (const [name, tag] of tags) assert.match(tag, /\s:subscriber="/, name + ": " + tag);

  const nick = parse(read(path.join(launcher, "NickName.vue"))).descriptor;
  assert.match(nick.template.content, /<span v-if="subscriber" class="nick-line"><img[^>]*class="nick-badge"[^>]*\/><span class="nick" :class="presetClass" :style="customCss"><slot><\/slot><\/span><\/span>/,
    "картинка — соседка .nick, а не её часть");
  assert.match(nick.template.content, /<span v-else class="nick" :class="presetClass" :style="customCss"><slot><\/slot><\/span>/, "без подписки разметка прежняя");
  assert.match(nick.template.content, /:alt="\$t\('#L_NICK_SUBSCRIBER'\)"/);
  assert.match(nick.template.content, /:title="badgeTitle"/);
  assert.match(nick.script.content, /\$t\("#L_NICK_SUBSCRIBER"\)/, "без даты — прежняя подсказка");
  assert.match(nick.script.content, /require\("@\/assets\/launcher\/subscriber-badge\.png"\)/, "у ника лямбда подписчика, не Nitro");
  const png = fs.readFileSync(path.join(__dirname, "../src/renderer/assets/launcher/subscriber-badge.png"));
  assert.equal(png.readUInt32BE(16), 128, "ширина значка");
  assert.equal(png.readUInt32BE(20), 128, "высота значка");
  assert.equal(png[25], 6, "RGBA: прозрачный фон");

  const css = read(path.join(__dirname, "../src/renderer/assets/css/launcher.css"));
  assert.match(css, /\.nick-badge \{[^}]*flex: none;[^}]*height: 1\.05em;[^}]*animation: nick-badge-glow /);
  assert.match(css, /@media \(prefers-reduced-motion: reduce\) \{\s*\.launcher-root \.nick-badge \{\s*animation: none !important;/);
  assert.match(css, /\.launcher-root\.no-anim \.nick-badge \{\s*animation: none !important;/);

  const ru = read(path.join(__dirname, "../src/renderer/utils/locales/ru.locale.js"));
  const en = read(path.join(__dirname, "../src/renderer/utils/locales/en.locale.js"));
  assert.ok(ru.includes("\"#L_NICK_SUBSCRIBER\": \"Подписчик HLSR\","));
  assert.ok(en.includes("\"#L_NICK_SUBSCRIBER\": \"HLSR subscriber\","));
});

test("центр уведомлений: ошибка прокси Chromium — подсказка вместо кода", () => {
  const { isProxyError } = require("../src/shared/proxy-error");
  for (const text of [
    "net::ERR_PROXY_CONNECTION_FAILED",
    "Error: net::ERR_TUNNEL_CONNECTION_FAILED",
    "net::ERR_MANDATORY_PROXY_CONFIGURATION_FAILED",
    "net::ERR_PROXY_AUTH_UNSUPPORTED (https://hlsr.pro/downloads/launcher/stable/latest.yml)",
  ])
    assert.equal(isProxyError(text), true, text);
  for (const value of ["net::ERR_CONNECTION_REFUSED", "net::ERR_NAME_NOT_RESOLVED", "", null, undefined, 42, "ERR_PROXY"])
    assert.equal(isProxyError(value), false, String(value));

  const read = (file) => fs.readFileSync(file, "utf8");
  const center = read(path.join(__dirname, "../src/renderer/components/Launcher/NotificationCenter.vue"));
  assert.match(center, /isProxyError\(item\.detail\)\)\s*return this\.\$t\(IS_WINDOWS \? "#L_N_PROXY_HINT" : "#L_N_PROXY_HINT_SYSTEM"\)/);
  for (const lang of ["ru", "en"]) {
    const locale = read(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`));
    assert.ok(locale.includes("\"#L_N_PROXY_HINT\":"), lang);
    assert.ok(locale.includes("\"#L_N_PROXY_HINT_SYSTEM\":"), lang);
  }
});

test("неоновая рамка: «Спонсорство» и профиль подписчика берут общий миксин, при уменьшенном движении анимации нет", () => {
  const dir = path.join(__dirname, "..", "src", "renderer", "components", "Launcher");
  const neon = fs.readFileSync(path.join(dir, "neon.scss"), "utf8");
  assert.ok(neon.includes("prefers-reduced-motion: reduce") && neon.includes("animation: none"));
  assert.ok(neon.includes(".launcher-root.no-anim .neon::before { animation: none !important; }"), "при выключенных анимациях кольцо не мерцает");
  assert.ok(!neon.includes("filter:"), "размытых слоёв по кадрам нет");
  for (const name of ["SponsorsModal", "ProfileModal"]) {
    const vue = fs.readFileSync(path.join(dir, name + ".vue"), "utf8");
    assert.ok(vue.includes("@include neon(") && vue.includes('class="neon"'), name);
  }
});
test("0.4.9: значки — ключи в ru и en, картинки на месте", () => {
  const dir = path.join(__dirname, "../src/renderer");
  const [ru, en] = fs.readFileSync(path.join(dir, "components/Launcher/runner-locales.js"), "utf8").split(/\n  en: \{/);
  for (const key of ["#L_RUNNER_BUGHUNTER", "#L_RUNNER_BOOSTER"]) {
    assert.ok(ru.includes(`"${key}"`), key + " ru");
    assert.ok(en.includes(`"${key}"`), key + " en");
  }
  for (const lang of ["ru", "en"])
    for (const key of ["#L_NICK_SUPPORTER_MONTHS", "#L_NICK_SUPPORTER_NEW"])
      assert.ok(fs.readFileSync(path.join(dir, `utils/locales/${lang}.locale.js`), "utf8").includes(`"${key}"`), key + " " + lang);
  for (const file of ["developer.png", "bughunter.png", "booster.png", "SOURCES.md"])
    assert.ok(fs.existsSync(path.join(dir, "assets/launcher/badges", file)), file);
  for (const file of ["supporter.png", "developer.svg"])
    assert.ok(!fs.existsSync(path.join(dir, "assets/launcher/badges", file)), file + " убран");
});

// Годовщина в HLSR: сервер присылает anniversary-N (N лет с регистрации, до 10).
test("значки годовщины: подписи со склонением, картинки 96×96 без фона, свечение по цвету картинки", () => {
  const anniversary = require("../src/shared/anniversary-badges");
  const { decodePng, dominantGlow } = require("./prepare-anniversary-badges");
  const dir = path.join(__dirname, "../src/renderer");
  const [ru, en] = fs.readFileSync(path.join(dir, "components/Launcher/runner-locales.js"), "utf8").split(/\n  en: \{/);
  const label = (block, lang, n) => {
    const key = anniversary.labelKey(n, lang);
    const match = new RegExp(`"${key}": "([^"]+)"`).exec(block);
    assert.ok(match, key + " " + lang);
    return match[1].replace("%0%", n);
  };
  const years = Array.from({ length: 10 }, (_, i) => i + 1);
  assert.deepEqual(years.map((n) => label(ru, "ru", n)), ["1 год в HLSR", "2 года в HLSR", "3 года в HLSR", "4 года в HLSR",
    "5 лет в HLSR", "6 лет в HLSR", "7 лет в HLSR", "8 лет в HLSR", "9 лет в HLSR", "10 лет в HLSR"]);
  assert.deepEqual(years.map((n) => label(en, "en", n)), ["1 year in HLSR"].concat(years.slice(1).map((n) => n + " years in HLSR")));

  assert.deepEqual(anniversary.IDS, years.map((n) => "anniversary-" + n));
  assert.equal(anniversary.GLOW[1], undefined, "у первого года свечения нет");
  // У 10 золота и фиолетового почти поровну: побеждает золото, и оно золотое, а не персиковое.
  const [r, g, b] = anniversary.GLOW[10].split(", ").map(Number);
  const hue = (60 * (g - b)) / (r - Math.min(g, b));
  assert.ok(r === 255 && hue >= 30 && hue <= 55 && (r - b) / r >= 0.6, "золото у 10: " + anniversary.GLOW[10]);
  for (const n of years) {
    const png = decodePng(fs.readFileSync(path.join(dir, `assets/launcher/badges/anniversary-${n}.png`)));
    assert.deepEqual([png.width, png.height], [96, 96], "размер как у соседних значков: " + n);
    for (const corner of [0, 95, 96 * 95, 96 * 96 - 1]) assert.equal(png.data[corner * 4 + 3], 0, `угол ${n} прозрачный`);
    if (n > 1) assert.equal(anniversary.GLOW[n], dominantGlow(png), "свечение " + n + " — преобладающий цвет картинки");
  }

  // Лаунчер показывает годовщины, неизвестные номера отсеивает.
  const profile = require("../src/main/online-profile/profile-state").normalize({ steamId64: "76561198000000001", personaName: "x",
    badges: ["verified", "anniversary-3", "anniversary-0", "anniversary-11"], games: {}, details: {} });
  assert.deepEqual(profile.badges, ["verified", "anniversary-3"]);

  const modal = parse(fs.readFileSync(path.join(dir, "components/Launcher/ProfileModal.vue"), "utf8")).descriptor;
  assert.match(modal.script.content, /anniversaryBadges\.IDS\.map\(\(id\) => require\("@\/assets\/launcher\/badges\/" \+ id \+ "\.png"\)\)/);
  assert.match(modal.script.content, /anniversary: true, glow: anniversaryBadges\.GLOW\[count\] \|\| null, label: this\.\$t\(anniversaryBadges\.labelKey\(count, lang\), String\(count\)\)/);
  assert.match(modal.template.content, /:class="\['is-' \+ badge\.id, \{ 'is-glow': badge\.glow, 'is-anniversary': badge\.anniversary \}\]"\s*:style="badge\.glow \? \{ '--glow': badge\.glow \} : null"/);
  const styles = modal.styles.map((style) => style.content).join("\n");
  assert.match(styles, /&\.is-booster > img,\s*&\.is-glow > img \{\s*filter: drop-shadow\(0 0 3px rgba\(var\(--glow\), 0\.55\)\);/);
  assert.match(styles, /&\.is-anniversary > img \{\s*width: 30px;\s*height: 30px;/, "квадрат годовщины ростом с соседние значки");
});

test("шаблон настроек профиля рисует раздел Discord и не содержит одинокого «+»", () => {
  const file = path.join(__dirname, "../src/renderer/components/Launcher/RunnerSettings.vue");
  const template = parse(fs.readFileSync(file, "utf8")).descriptor.template.content;
  for (const key of ["LINK", "UNLINK", "COPY", "STEPS", "STEP_CODE", "STEP_OPEN", "OPEN", "STEP_TYPE", "STEP_CHECK", "STEP_BACK", "UNTIL", "NOT_LINKED"])
    assert.ok(template.includes("#L_RUNNER_DISCORD_" + key), key);
  assert.ok(!template.split(/\r?\n/).some((line) => line.trim() === "+"));
  assert.ok(!template.includes("v-html"));
});

test("сводка профиля: колонки и строки сетки заданы явно, а не потоком", () => {
  const vue = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/ProfileModal.vue"), "utf8");
  assert.doesNotMatch(vue, /grid-auto-flow:\s*column/);
  for (const rule of [
    "&__column:nth-child(1) > * { grid-column: 1; }",
    "&__column:nth-child(2) > * { grid-column: 2; }",
    "&__column > :nth-child(1) { grid-row: 1; }",
    "&__column > :nth-child(2) { grid-row: 2; }",
  ]) assert.ok(vue.includes(rule), rule);
});

test("панель персонализации: при CONFIG_CHANGED черновик не заменяется, есть «Перечитать» и отмена", () => {
  const source = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GamePersonalization.vue"), "utf8");
  const handler = /error\.code === "CONFIG_CHANGED"\) \{([\s\S]*?)\n          \}/.exec(source);
  assert.ok(handler, "обработчик CONFIG_CHANGED есть");
  assert.ok(handler[1].includes("this.conflictGameId = id"));
  assert.ok(!/\bload\(/.test(handler[1]), "черновик в обработчике не перечитывается");
  assert.match(source, /@click="reloadAfterChange"/);
  assert.match(source, /async reloadAfterChange\(\) \{[\s\S]*?await this\.load\(id\)/);
  const reload = source.slice(source.indexOf("async reloadAfterChange"), source.indexOf("async save()"));
  assert.ok(reload.includes("const id = this.conflictGameId") && !reload.includes("this.gameId"), "перечитывается игра конфликта, не активная вкладка");
  assert.match(source, /conflictShown\(\) \{\s*return !!this\.conflictGameId && this\.conflictGameId === this\.gameId && !this\.invalid\[this\.gameId\]/);
  assert.equal((source.match(/conflictShown/g) || []).length, 3, "сообщение и кнопки по одному условию");
  assert.ok(source.includes(`@click="conflictGameId = ''"`));
  assert.ok(source.includes("#L_GS_CFG_CHANGED_RELOAD") && source.includes("#L_CANCEL"));
});

test("Персонализация: нет раздела personal и старой панели, тема лаунчера — в «Интерфейсе» настроек", () => {
  const dir = path.join(__dirname, "../src/renderer");
  const read = (file) => fs.readFileSync(path.join(dir, file), "utf8");
  assert.doesNotMatch(read("components/Launcher/LauncherShell.vue"), /["']personal["']/, "в оболочке нет раздела personal");
  assert.ok(!fs.existsSync(path.join(dir, "components/Launcher/PersonalizationPanel.vue")), "старой панели нет");
  for (const file of fs.readdirSync(path.join(dir, "components/Launcher")))
    if (file.endsWith(".vue")) assert.ok(!read("components/Launcher/" + file).includes("PersonalizationPanel"), file + " не упоминает PersonalizationPanel");
  assert.ok(read("components/Launcher/SettingsModal.vue").includes("<LauncherThemePicker"), "сетка тем в настройках");
  for (const lang of ["ru", "en"]) {
    const locale = read(`utils/locales/${lang}.locale.js`);
    for (const key of ["#L_PZ_THEMES_LEAD", "#L_TAB_PERSONAL"]) assert.ok(!locale.includes(`"${key}"`), `${lang}: ${key} удалён`);
  }
});
test("вкладка «Конфиги»: в шапке между «Играми» и «Персонализацией», окно настроек игры встроено, а не модальное", () => {
  const dir = path.join(__dirname, "../src/renderer");
  const read = (file) => fs.readFileSync(path.join(dir, file), "utf8");
  const shell = read("components/Launcher/LauncherShell.vue");
  const ids = [...shell.matchAll(/\{ id: "(\w+)", title: "(#L_TAB_\w+)" \}/g)].map((m) => m[1] + ":" + m[2]);
  assert.deepEqual(ids, ["home:#L_TAB_HOME", "games:#L_TAB_GAMES", "configs:#L_TAB_CONFIGS"]);
  const template = parse(shell).descriptor.template.content;
  assert.ok(!template.includes("<GameSettingsModal"), "модальное окно из оболочки убрано");
  assert.match(template, /<GameConfigsPanel[\s\S]*?v-if="section === 'configs'"/);
  assert.match(shell, /openGameSettings\(\) \{\s*this\.configsGameId = this\.selectedGameId;\s*this\.section = "configs";\s*this\.configsOpenToken \+= 1;/);

  const panel = read("components/Launcher/GameConfigsPanel.vue");
  for (const channel of ["player-config-import-pick", "player-config-import-apply", "player-config-import-undo", "player-config-export", "game-config-open-folder"])
    assert.ok(panel.includes(`"${channel}"`), channel);
  for (const anchor of ["configs-games", "configs-import", "configs-export", "configs-help"])
    assert.ok(panel.includes(`data-tour="${anchor}"`), anchor);
  assert.match(panel, /<GameSettingsModal[\s\S]*?embedded/);
  assert.ok(panel.includes("#L_CFG_UNSAVED_STAY") && panel.includes("#L_CFG_NO_GAMES"));
  // Стиль .settings-btn у каждого окна свой (scoped): у панели он должен быть описан сам.
  assert.match(panel, /\n\.settings-btn \{[\s\S]*?min-height: 48px/, "у панели есть собственный стиль .settings-btn");
  assert.equal((panel.match(/<button[^>]*?class="settings-btn(?: primary)?"/g) || []).length, 7, "кнопки нижней строки, пустого экрана и диалога — settings-btn");
  // Импорт, экспорт и папка — в нижней строке окна (слот), не в шапке; «?» сразу за заголовком.
  const head = panel.slice(panel.indexOf('<header class="cfgp__head"'), panel.indexOf("</header>"));
  assert.ok(!/configs-import|configs-export|importConfig|exportConfig|openFolder/.test(head), "в шапке нет импорта, экспорта и папки");
  assert.ok(head.indexOf("cfgp__title") < head.indexOf('data-tour="configs-help"') && head.indexOf('data-tour="configs-help"') < head.indexOf("cfgp__games"), "«?» между заголовком и выбором игры");
  const footerSlot = panel.slice(panel.indexOf("#footer-start"), panel.indexOf("</GameSettingsModal>"));
  const slotOrder = ["configs-import", "configs-export", "openFolder"].map((s) => footerSlot.indexOf(s));
  assert.ok(slotOrder.every((i) => i > 0) && slotOrder[0] < slotOrder[1] && slotOrder[1] < slotOrder[2], "слева внизу: импорт, экспорт, папка");
  assert.ok(/class="settings-btn cfgp__compact"[^>]*@click="undoImport"/.test(panel), "«Вернуть прежний конфиг» — компактная кнопка");
  const gsm = read("components/Launcher/GameSettingsModal.vue");
  assert.ok(gsm.includes('<slot name="footer-start"></slot>'), "слот левой группы подвала");
  assert.ok(/\.game-settings__window \{[^}]*background: none;[^}]*border-radius: 0;[^}]*box-shadow: none;/.test(gsm.slice(gsm.indexOf(".settings-modal.game-settings--embedded"))), "во вкладке нет внешней рамки");
  const footerEnd = gsm.slice(gsm.indexOf('<div class="game-settings__footer-end">'), gsm.indexOf("</footer>"));
  assert.ok(footerEnd.indexOf('v-if="embedded" type="button" class="settings-btn ghost" @click="resetDraft"') > 0
    && footerEnd.indexOf("resetDraft") < footerEnd.indexOf('@click="onCancel"') && footerEnd.indexOf('@click="onCancel"') < footerEnd.indexOf("submit()"), "справа: Сбросить, Отмена, Сохранить");
  assert.ok(read("utils/locales/ru.locale.js").includes("(%0%)") && !/userconfig\.cfg или autoexec\.cfg\)/.test(read("utils/locales/ru.locale.js")), "в описании раздела имя файла игры");
  assert.ok(/class="settings-btn primary" @click="answer\('save'\)"/.test(panel), "«Сохранить» — primary");
  assert.ok(read("components/Launcher/GamePersonalization.vue").includes('<div class="pz-editor">'), "якорь обучения переехал на пункт списка слева");
  assert.ok(read("components/Launcher/GameSettingsModal.vue").includes(`:data-tour="'gs-' + s.id"`), "якорь gs-personal на пункте списка");
  const embeddedModal = read("components/Launcher/GameSettingsModal.vue");
  assert.match(embeddedModal, /<button\s+v-if="!embedded"\s+type="button"\s+class="settings-btn"\s+:disabled="!configInfo/, "«Открыть папку» окна скрыта во вкладке");
  assert.ok(embeddedModal.includes(`v-if="!embedded && activeSection === 'configs' && configState`), "«Импорт из cfg» окна скрыт во вкладке");
  assert.ok(/\.game-settings__tabs \{\s*flex-direction: column;[^}]*margin-top: 0;/.test(embeddedModal), "разделы списком слева, без отступа сверху");
  assert.ok(/&__label \{[^}]*overflow-wrap: break-word; word-break: normal; hyphens: none/.test(read("components/Launcher/settings-fields.scss")), "подписи не рвутся посреди слова");
  assert.ok(/&__label \{ flex: 1; min-width: 0;/.test(read("components/Launcher/settings-fields.scss")) && !/min-width: min-content/.test(read("components/Launcher/settings-fields.scss")), "подпись может сжиматься (длинные токены не распирают строку)");

  const modal = read("components/Launcher/GameSettingsModal.vue");
  assert.match(modal, /embedded: \{\s*type: Boolean/);
  assert.ok(modal.includes(`:data-tour="'gs-' + s.id"`), "якорь вкладки окна");
  assert.match(modal, /<button\s+v-if="!embedded"\s+type="button"\s+class="game-settings__close"/);
  assert.equal((modal.match(/@click="onCancel"/g) || []).length, 2, "«Отмена» в обоих подвалах");
  assert.match(modal, /onCancel\(\) \{\s*if \(this\.embedded\) this\.discardChanges\(\);\s*else this\.\$emit\("close"\);/);

  assert.match(shell, /data-tour="tab\.id === 'configs'|:data-tour="tab\.id === 'configs' \? 'configs-tab' : null"/);
  assert.match(panel, /openToken\(\) \{\s*if \(!this\.initialGameId\) return;[^]*?gs\.selectSection\(this\.initialSection\);\s*else this\.requestGame\(this\.initialGameId, this\.initialSection\);/, "каждое нажатие шестерёнки доходит до панели, на открытой игре меняет раздел (M2)");
  assert.match(panel, /deactivated\(\) \{\s*this\.answer\("stay"\);/, "уход с вкладки закрывает вопрос");
  assert.ok(panel.includes('import { trapFocus } from "./focus-trap"') && panel.includes("trapFocus(this.$refs.dialog, e)"), "ловушка фокуса");
  assert.match(panel, /e\.key === "Escape"[\s\S]*?this\.answer\("stay"\)/);
  assert.equal((panel.match(/:inert="unsaved \? true : null"/g) || []).length, 2, "фон недоступен, пока открыт вопрос");
  assert.ok(!/Сначала установите/.test(read("utils/locales/ru.locale.js")), "обращение на «ты»");
  assert.ok(read("utils/locales/en.locale.js").includes('"#L_CFG_IMPORT_UNDO": "Restore previous config"'));
  const keys = ["#L_TAB_CONFIGS", "#L_CFG_IMPORT", "#L_CFG_EXPORT", "#L_CFG_OPEN_FOLDER", "#L_CFG_HELP", "#L_CFG_NO_GAMES",
    "#L_CFG_IMPORT_DONE", "#L_CFG_IMPORT_UNDO", "#L_CFG_UNSAVED_TITLE", "#L_CFG_UNSAVED_SAVE", "#L_CFG_UNSAVED_DISCARD", "#L_CFG_UNSAVED_STAY"];
  for (const lang of ["ru", "en"]) {
    const locale = read(`utils/locales/${lang}.locale.js`);
    for (const key of keys) assert.ok(locale.includes(`"${key}"`), key + " " + lang);
  }
});

test("опрос первого запуска: до запуска игры, окно с тремя кнопками, передача во вкладку «Конфиги»", () => {
  const root = path.join(__dirname, "../src/renderer");
  const read = (p) => fs.readFileSync(path.join(root, p), "utf8");
  const shell = read("components/Launcher/LauncherShell.vue");
  const modal = read("components/Launcher/ConfigSetupModal.vue");
  const panel = read("components/Launcher/GameConfigsPanel.vue");

  const play = shell.slice(shell.indexOf("async onPlay()"));
  assert.ok(play.indexOf("this.offerConfigSetup(gameId)") > 0, "опрос зовётся из onPlay");
  assert.ok(play.indexOf("this.offerConfigSetup(gameId)") < play.indexOf('"game-process-launch"'), "до game-process-launch");
  assert.ok(play.indexOf("this.offerConfigSetup(gameId)") < play.indexOf("this.offerTimer()"), "до предложения таймера");
  assert.match(shell, /<ConfigSetupModal[\s\S]*?@choose="onConfigSetupChoose"[\s\S]*?@close="onConfigSetupChoose\(null\)"/);
  assert.match(shell, /if \(choice === "scratch"\) this\.configTourRequested = true;\s*else this\.configsImportRequested = true;/);
  assert.match(shell, /if \(choice === "skip"\) return false;/);
  // Двойной клик: второй вызов выходит сразу, флаг снимается в finally.
  assert.match(shell, /async offerConfigSetup\(gameId\) \{[^]*?if \(this\.configSetupBusy\) return true;\s*this\.configSetupBusy = true;\s*try \{/);
  assert.match(shell, /\} finally \{\s*this\.configSetupBusy = false;/);
  // Сбой setup-done не роняет запуск.
  assert.match(shell, /try \{\s*await ipcRenderer\.invoke\("player-config-setup-done", gameId, choice\);\s*\} catch \(error\) \{\s*console\.error/);
  // Запрос импорта принимается один раз.
  assert.ok(shell.includes(':importRequested="configsImportRequested"') && shell.includes('@import-started="configsImportRequested = false"'));
  assert.match(panel, /importRequested\(value\) \{\s*if \(value\) this\.startImport\(\);/);
  assert.match(panel, /mounted\(\) \{[\s\S]*?if \(this\.importRequested\) this\.startImport\(\);/);
  assert.match(panel, /async startImport\(\) \{\s*this\.\$emit\("import-started"\);/);
  // Ждём событие окна, а не таймер; при сбое загрузки — подсказка.
  assert.ok(!/setTimeout/.test(panel), "в панели нет опроса по таймеру");
  assert.ok(panel.includes('@ready="settle(true)"') && panel.includes('@load-failed="settle(false)"') && panel.includes("#L_CFG_IMPORT_NOT_READY"));
  const gsModal = read("components/Launcher/GameSettingsModal.vue");
  assert.ok(gsModal.includes('loadState: "loading"'));
  assert.match(gsModal, /this\.load\(\)\.then\(\s*\(\) => \{\s*this\.loadState = "ready";\s*this\.\$emit\("ready"\);[\s\S]*?this\.loadState = "failed";\s*this\.\$emit\("load-failed"\);/);
  // Состояние читается до ожидания; ждём событие только при "loading"; уход и смена игры отменяют ожидание.
  const wait = panel.slice(panel.indexOf("async settingsReady()"));
  assert.ok(wait.indexOf('gs.loadState === "failed"') > 0 && wait.indexOf('gs.loadState === "failed"') < wait.indexOf("new Promise"));
  assert.ok(wait.indexOf('gs.loadState === "ready"') < wait.indexOf("new Promise"));
  assert.match(panel, /deactivated\(\) \{\s*this\.answer\("stay"\);\s*this\.finishWait\("cancelled"\);/);
  assert.match(panel, /gameId\(\) \{\s*this\.finishWait\("cancelled"\);/);
  assert.match(panel, /state === "cancelled" \|\| this\.gameId !== id\) return;/);
  for (const lang of ["ru", "en"]) assert.ok(read(`utils/locales/${lang}.locale.js`).includes('"#L_CFG_IMPORT_NOT_READY"'));

  for (const choice of ["scratch", "own", "skip"]) assert.ok(modal.includes(`$emit('choose', '${choice}')`), choice);
  assert.ok(modal.includes('import { trapFocus } from "./focus-trap"') && modal.includes("trapFocus(this.$refs.window, e)"));
  assert.match(modal, /e\.key === "Escape"[\s\S]*?this\.\$emit\("close"\)/);
  assert.ok(modal.includes("prefers-reduced-motion") && modal.includes(".no-anim"));
  assert.ok(read("../main/preload.js").includes('"player-config-setup-state"') && read("../main/preload.js").includes('"player-config-setup-done"'));

  const keys = ["#L_CFG_SETUP_TITLE", "#L_CFG_SETUP_TEXT", "#L_CFG_SETUP_SCRATCH", "#L_CFG_SETUP_OWN", "#L_CFG_SETUP_SKIP"];
  for (const lang of ["ru", "en"]) {
    const locale = read(`utils/locales/${lang}.locale.js`);
    for (const key of keys) assert.ok(locale.includes(`"${key}"`), key + " " + lang);
  }
});


test("раздел «Демо» есть у HL1 и Ghosting, пакует через IPC и не трогает fs", () => {
  const dir = path.join(__dirname, "../src/renderer/components/Launcher");
  const modal = fs.readFileSync(path.join(dir, "GameSettingsModal.vue"), "utf8");
  const demos = fs.readFileSync(path.join(dir, "GameDemos.vue"), "utf8");
  assert.match(modal, /\{ id: "demos"[^}]*demosOnly: true/);
  assert.match(modal, /hasDemos\(\) \{\s*return this\.isGoldSrc \|\| this\.gameId === GHOSTING_GAME_ID;/);
  assert.ok(modal.includes('<GameDemos') && modal.includes(":active=\"activeSection === 'demos'\""));
  assert.ok(demos.includes('"game-demos-pack"') && !/require\(["']fs["']\)|from ["']fs["']/.test(demos));
  assert.ok(demos.includes(':disabled="!chosenCount || packing"') && demos.includes('role="alert"'));
  for (const key of ["#L_DEMOS_PACK", "#L_DEMOS_ERR_CHANGED", "#L_DEMOS_ERR_PACK", "#L_DEMOS_EMPTY"])
    for (const lang of ["ru", "en"])
      assert.ok(fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8").includes(`"${key}"`), key + " " + lang);
});

test("страница «Персонализация»: разделы слева по порядку, клавиатура на месте, Half-Life 2 «в разработке»", () => {
  const dir = path.join(__dirname, "../src/renderer/components/Launcher");
  const read = (name) => fs.readFileSync(path.join(dir, name), "utf8");
  const modal = read("GameSettingsModal.vue");
  const list = /allSections: \[([\s\S]*?)\n      \],/.exec(modal);
  assert.ok(list, "allSections");
  assert.deepEqual([...list[1].matchAll(/\{ id: "(\w+)"/g)].map((m) => m[1]),
    ["personal", "configs", "keyboard", "scripts", "launch", "demos", "ghosts", "version"]);
  assert.match(list[1], /\{ id: "keyboard"[^}]*window: !this\.embedded \}/, "окно клавиатуры только вне страницы");
  assert.match(list[1], /\{ id: "personal"[^}]*personalOnly: true \}/);
  assert.match(modal, /\(!item\.personalOnly \|\| this\.hasPersonal\)/);
  assert.match(modal, /hasPersonal\(\) \{\s*return this\.embedded && \(this\.isGoldSrc \|\| this\.gameId === GHOSTING_GAME_ID\);/, "только на странице, HL1 и Ghosting");
  assert.ok(modal.includes('v-if="hasKeyboard && !embedded"'), "окно клавиатуры на странице не собирается");
  assert.match(modal, /v-if="embedded && hasKeyboard"[^>]*>\s*<GameKeyboard/, "клавиатура прямо в теле раздела");
  assert.match(modal, /<GamePersonalization[\s\S]*?@dirty="personalDirty = \$event"/);
  assert.ok(modal.includes(`<footer v-if="activeSection !== 'personal'"`), "у «Персонализации» свои «Вернуть» и «Сохранить»");
  assert.ok(read("GameConfigEditor.vue").includes('class="gx gx--wide gc gc--top"'), "подразделы конфига строкой сверху");
  assert.match(modal, /measureInk\(motion\) \{\s*if \(this\.embedded\) return;/, "у списка слева нет полосы под вкладкой");
  assert.match(modal, /section: this\.initialSection,/);
  const editor = read("GameConfigEditor.vue");
  assert.ok(!editor.includes("navLayout"), "ветки «side» больше нет (M8)");

  const panel = read("GameConfigsPanel.vue");
  // I1: чипы строит configChips; неактивный — aria-disabled, без перехода на игру, с подсказкой «Скоро».
  assert.ok(panel.includes('v-for="chip in chips"') && panel.includes("chip.disabled || requestGame(chip.id)"), "чипы из configChips");
  assert.ok(panel.includes(':key="chip.id || chip.name"'), "у неактивных HL2 и LiveSplit (id пустой) свои ключи");
  assert.ok(panel.includes(":aria-disabled=\"chip.disabled ? 'true' : null\"") && panel.includes("#L_BUILD_SOON"), "неактивен, подсказка «Скоро»");
  assert.ok(panel.includes(":initial-section=\"openSection\"") && panel.includes("#L_CFG_HL2_SOON"));
  for (const lang of ["ru", "en"])
    assert.ok(fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8").includes('"#L_CFG_HL2_SOON"'), lang);
  // Шестерёнка, опрос и обучение открывают «Конфиг», вкладка шапки — первый раздел.
  const shell = read("LauncherShell.vue");
  assert.equal((shell.match(/this\.configsSection = "configs";/g) || []).length, 3);
  assert.ok(shell.includes(':initialSection="configsSection"'));
});

test("Review Focus 4: несохранённая «Персонализация» — тот же вопрос при смене раздела слева и игры", async () => {
  // Окно: уход из «Персонализации» спрашивает о ней, переход в неё — о черновике окна, между остальными — без вопроса.
  const select = method("selectSection", undefined, {}, "GameSettingsModal");
  const asked = [];
  let answer = false;
  const modal = { activeSection: "personal", section: "personal", askUnsaved: async (part) => { asked.push(part); return answer; } };
  assert.equal(await select.call(modal, "configs"), false);
  assert.equal(modal.section, "personal", "«Остаться» — раздел прежний");
  answer = true;
  assert.equal(await select.call(modal, "configs"), true);
  assert.equal(modal.section, "configs");
  modal.activeSection = "configs";
  await select.call(modal, "launch");
  await select.call(modal, "personal");
  assert.deepEqual(asked, ["personal", "personal", "settings"]);

  // Панель: что считается несохранённым.
  const unsavedParts = method("unsavedParts", undefined, {}, "GameConfigsPanel");
  const dirty = method("dirty", undefined, {}, "GameConfigsPanel");
  const page = (gs) => {
    const vm = { settings: () => gs };
    vm.unsavedParts = unsavedParts.bind(vm);
    vm.dirty = dirty.bind(vm);
    return vm;
  };
  const clean = { draftSaved: "{}", hasUnsaved: () => false, personalDirty: false };
  const pzDirty = page({ ...clean, personalDirty: true });
  assert.deepEqual([page(clean).dirty(), pzDirty.dirty(), pzDirty.dirty("personal"), pzDirty.dirty("settings")], [false, true, true, false]);
  assert.equal(page({ ...clean, hasUnsaved: () => true }).dirty("personal"), false);
  assert.equal(page({ ...clean, draftSaved: null, hasUnsaved: () => true }).dirty(), false, "окно ещё читает настройки");
  assert.equal(page(null).dirty(), false);

  // Вопрос: «Сохранить» — кнопкой персонализации, «Не сохранять» — её «Вернуть»; черновик окна не трогается.
  const settle = method("settleUnsaved", undefined, {}, "GameConfigsPanel");
  const ask = async (choice, part, overrides = {}) => {
    const calls = [];
    const pz = {
      save: async () => { calls.push("pz.save"); gs.personalDirty = false; },
      revertGame: () => { calls.push("pz.revert"); gs.personalDirty = false; },
    };
    const gs = Object.assign({ draftSaved: "{}", hasUnsaved: () => false, personalDirty: true, fileChanged: false, cfgFilesDirty: false,
      $refs: { personal: pz }, submit: async () => calls.push("submit"), discardChanges: async () => calls.push("discard") }, overrides);
    const vm = Object.assign(page(gs), { unsaved: null, $refs: {}, $nextTick: (fn) => (fn ? fn() : Promise.resolve()) });
    const pending = settle.call(vm, part);
    await new Promise(setImmediate);
    assert.ok(vm.unsaved, "вопрос открыт");
    vm.unsaved.resolve(choice);
    return [await pending, calls];
  };
  assert.deepEqual(await ask("save", "personal"), [true, ["pz.save"]]);
  assert.deepEqual(await ask("discard", "personal"), [true, ["pz.revert"]]);
  assert.deepEqual(await ask("stay", "personal"), [false, []]);
  assert.deepEqual(await ask("save"), [true, ["pz.save"]], "смена игры: сохраняется только то, что правили");
  assert.deepEqual(await ask("save", "personal", { $refs: { personal: { save: async () => {} } } }), [false, []], "не сохранилось — остаёмся");
  assert.deepEqual(await ask("discard", "settings", { hasUnsaved: () => true }), [true, ["discard"]], "переход в «Персонализацию» спрашивает только о черновике окна");

  // «Не сохранять» после CONFIG_CHANGED: сообщение о конфликте и ошибка уходят вместе с черновиком.
  const pz = { codeImported: "x", conflictGameId: "70", saveError: "err", draft: { reset: true, saved: '{"a":1}', look: { a: 2 } } };
  method("revertGame", undefined, {}, "GamePersonalization").call(pz);
  assert.deepEqual([pz.conflictGameId, pz.saveError, pz.draft.look], ["", "", { a: 1 }]);
});

test("R6, R7: подразделы «Персонализации» — под пунктом слева, без рамок внутри, нижняя строка одной высоты", () => {
  const dir = path.join(__dirname, "../src/renderer/components/Launcher");
  const read = (name) => fs.readFileSync(path.join(dir, name), "utf8");
  // Редактор: своего списка нет, подраздел приходит снаружи и выбирается, список уходит событием.
  const extras = read("GhostingSettingsExtras.vue");
  assert.ok(!extras.includes("hideNav") && !extras.includes("onNavKey") && extras.includes('class="gx gx--no-nav"'), "своего списка у редактора нет (M8)");
  // Между полями и предпросмотром — разделитель ширины (PaneSplitter), колонки зажаты в его пределы.
  assert.match(extras, /\.gx--no-nav \{\s*grid-template-columns: clamp\(340px, var\(--gx-fields\), calc\(100% - 412px\)\) 12px minmax\(0, 1fr\);/);
  assert.match(extras, /<PaneSplitter v-if="hasPreview" v-model="fieldsWidth" :storage-key="'pz-fields:' \+ gameId" :min="340" :rest="400"/);
  assert.match(extras, /navState: \{ immediate: true, handler\(value\) \{ this\.\$emit\('nav', value\); \} \}/);
  const pick = method("section", undefined, {}, "GhostingSettingsExtras");
  const editor = { activeId: "personalization", picked: [], selectSection(id) { this.picked.push(id); this.activeId = id; } };
  pick.call(editor, "hud");
  pick.call(editor, "hud");
  pick.call(editor, "");
  assert.deepEqual(editor.picked, ["hud"], "выбор со страницы — один раз, пустое значение не трогает");
  const pz = read("GamePersonalization.vue");
  assert.match(pz, /<GhostingSettingsExtras[\s\S]*?:section="section"[\s\S]*?@nav="\$emit\('nav', \$event\)"/);
  assert.match(pz, /:deep\(\.pers\),\s*:deep\(\.gx-pane\),\s*:deep\(\.look-code\) \{\s*border: 1px solid var\(--l-border\);\s*background: none;/, "колонка настроек с тонкой рамкой, без подложки");
  assert.match(pz, /\.pz-btn \{[^}]*min-height: 48px;/, "кнопки как у остальных разделов");
  // Страница: подразделы под пунктом, пока он открыт; выбор уходит в редактор.
  const modal = read("GameSettingsModal.vue");
  assert.match(modal, /v-if="s\.id === 'personal' && activeSection === 'personal' && personalNav\.items\.length"/);
  assert.ok(modal.includes('@click="personalNav.active = item.id"') && modal.includes(':section="personalNav.active"') && modal.includes('@nav="personalNav = $event"'));
  // Нижняя строка не переносится: правая группа и заметка сжимаются, «Убрать импорт» — значком.
  assert.ok(modal.includes(".game-settings__footer > .settings-modal__footer-spacer { display: none; }"));
  assert.ok(modal.includes(".game-settings__footer-end > .game-settings__note { flex: 1 1 0; min-width: 120px; }"));
  assert.match(modal, /class="settings-btn ghost game-settings__icon-btn"[\s\S]*?:aria-label="\$t\('#L_KB_IMPORT_REMOVE'\)"/);
  assert.ok(fs.readFileSync(path.join(__dirname, "../src/renderer/assets/css/launcher.css"), "utf8").includes('[data-theme="liquid-glass"] .game-settings__subtab:not(.is-active)'));
});

test("компактная клавиатура: только основной блок, полная раскладка не изменилась", () => {
  // keyboard-layout.js — ES-модуль без package type: превращаем единственный export в module.exports.
  const src = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/keyboard-layout.js"), "utf8");
  assert.ok(/^export \{[^}]*\};?\s*$/m.test(src));
  const mod = { exports: {} };
  new Function("module", src.replace(/^export \{([^}]*)\};?\s*$/m, "module.exports = {$1};"))(mod);
  const { layoutFor, MAIN_WIDTH, BOARD_WIDTH } = mod.exports;
  assert.equal(MAIN_WIDTH, 15);
  for (const engine of ["goldsrc", "source"]) {
    const full = layoutFor(engine), main = layoutFor(engine, { full: false });
    assert.equal(layoutFor(engine, { full: true }), full, "по умолчанию — вся раскладка");
    assert.equal(full.width, BOARD_WIDTH);
    assert.equal(main.width, MAIN_WIDTH);
    const names = (layout) => new Set(layout.keys.map((cap) => cap.key));
    for (const key of ["ESCAPE", "F12", "SPACE", "BACKSPACE", "ENTER", "TAB", "Q"].map((name) => (name === "Q" ? "q" : name))) {
      assert.ok(names(main).has(key), `${engine}: в основном блоке нет ${key}`);
    }
    for (const key of ["INS", "HOME", "UPARROW", "LEFTARROW", "KP_ENTER", "KP_5", "KP_INS", "KP_SLASH", "MOUSE1", "MOUSE5", "MWHEELUP"]) {
      assert.ok(names(full).has(key), `${engine}: в полной раскладке нет ${key}`);
      assert.ok(!names(main).has(key), `${engine}: в компактной раскладке лишняя ${key}`);
    }
    assert.ok(main.keys.every((cap) => cap.x + cap.w <= MAIN_WIDTH));
    assert.ok(main.keys.length < full.keys.length);
    const ids = new Set(full.keys.map((cap) => cap.id));
    assert.ok(main.keys.every((cap) => ids.has(cap.id)), "компактная — часть полной");
    assert.equal(full.keys.filter((cap) => cap.x + cap.w <= MAIN_WIDTH).length, main.keys.length);
  }
});
// Окно ремонта (RepairModal.vue): отметки по умолчанию, папки, счётчик, cfg.
function repairModal(files) {
  const m = (name) => method(name, undefined, undefined, "RepairModal");
  const vm = instance({
    data: Object.assign({ files, emitted: [] }, m("data").call({ files })),
    computed: { sections: m("sections"), chosenFiles: m("chosenFiles"), chosenBytes: m("chosenBytes"), warnConfigs: m("warnConfigs") },
    methods: Object.fromEntries(["groupsOf", "sizeOf", "toggle", "setAll", "groupState", "setGroup", "onResolve"].map((name) => [name, m(name)])),
  });
  vm.$t = (key, ...args) => [key, ...args].join("|");
  vm.$emit = (event, value) => vm.emitted.push([event, value]);
  return vm;
}

test("окно ремонта: отмечены недостающие и повреждённые, изменённые cfg — нет; предупреждение и ответ списком", () => {
  const vm = repairModal([
    { path: "bin/engine.dll", size: 1000, kind: "missing" },
    { path: "hl2.exe", size: 500, kind: "damaged" },
    { path: "bin/dxsupport.cfg", size: 10, kind: "cfg-missing" },
    { path: "bin/dxsupport_sp.cfg", size: 20, kind: "cfg-changed" },
  ]);
  assert.deepEqual(vm.chosenFiles.map((file) => file.path), ["bin/engine.dll", "hl2.exe", "bin/dxsupport.cfg"]);
  assert.equal(vm.chosenBytes, 1510);
  assert.equal(vm.warnConfigs, false);
  assert.deepEqual(vm.sections.map((section) => [section.id, section.title, section.groups.length, section.groups[0].folder]),
    [["build", null, 1, false], ["cfg", "#L_GM_REPAIR_CFG", 1, false]]);

  vm.toggle("bin/dxsupport_sp.cfg");
  assert.equal(vm.warnConfigs, true, "отмеченный изменённый cfg — предупреждение");
  assert.equal(vm.chosenBytes, 1530);

  vm.setAll(false);
  assert.deepEqual(vm.chosenFiles, []);
  vm.toggle("hl2.exe");
  vm.onResolve(true);
  vm.onResolve(false);
  assert.deepEqual(vm.emitted, [["resolve", ["hl2.exe"]], ["resolve", false]]);
});

test("окно ремонта: больше 30 файлов — папки с одной галочкой на папку", () => {
  const files = Array.from({ length: 40 }, (_, i) => ({ path: `ghostingmod/maps/m${i}.bsp`, size: 1, kind: "missing" }))
    .concat([{ path: "hl2.exe", size: 5, kind: "damaged" }]);
  const vm = repairModal(files);
  const groups = vm.sections[0].groups;
  assert.deepEqual(groups.map((group) => [group.key, group.folder, group.files.length]), [["", true, 1], ["ghostingmod/maps", true, 40]]);
  const maps = groups[1];
  assert.equal(vm.groupState(maps), "all");
  vm.setGroup(maps, false);
  assert.equal(vm.groupState(maps), "none");
  assert.deepEqual(vm.chosenFiles.map((file) => file.path), ["hl2.exe"]);
  vm.toggle("ghostingmod/maps/m3.bsp");
  assert.equal(vm.groupState(maps), "some");
  assert.equal(vm.chosenBytes, 6);
  vm.setAll(true);
  assert.equal(vm.chosenFiles.length, 41);
});

test("«Восстановить игру»: в ремонт уходят только отмеченные файлы, итог по повторной проверке", async () => {
  const calls = [], notes = [], asked = [];
  let answer = { status: "repaired", repaired: ["bin/engine.dll"], remaining: [] };
  const ipc = { invoke: async (channel, id, options) => {
    calls.push([channel, id, options]);
    return options ? answer : { status: "confirmation-required", damaged: 2, files: [{ path: "bin/engine.dll", size: 1, kind: "missing" }, { path: "hl2.exe", size: 1, kind: "damaged" }] };
  } };
  const shell = {
    $t: (key, ...args) => [key, ...args].join("|"),
    notify: (text) => notes.push(text),
    notifyDeliveryError: (result) => notes.push("error:" + (result && result.error && result.error.code)),
    askRepair: async (files) => { asked.push(files.map((file) => file.path)); return ["bin/engine.dll"]; },
    askConfirm: async (request) => { asked.push(request); return false; },
  };
  shell.runMenuAction = method("runMenuAction", undefined, ipc).bind(shell);
  shell.notifyRepairError = method("notifyRepairError").bind(shell);
  shell.stockCfgParts = method("stockCfgParts").bind(shell);
  shell.stockCfgSentence = method("stockCfgSentence").bind(shell);
  shell.onRepairGame = method("onRepairGame", undefined, ipc).bind(shell);

  await shell.onRepairGame("ghosting-renewed");
  assert.deepEqual(calls, [
    ["game-menu-repair", "ghosting-renewed", undefined],
    ["game-menu-repair", "ghosting-renewed", { confirmed: true, recheck: false, files: ["bin/engine.dll"] }],
  ]);
  assert.deepEqual(asked, [["bin/engine.dll", "hl2.exe"]]);
  assert.deepEqual(notes, ["#L_GM_REPAIR_DONE_N|1"]);

  answer = { status: "repaired", repaired: ["bin/engine.dll"], remaining: ["hl2.exe"] };
  await shell.onRepairGame("ghosting-renewed");
  assert.equal(asked[2].text, "#L_GM_REPAIR_RESULT|1|1");
  assert.deepEqual(asked[2].items, ["hl2.exe"]);

  notes.length = 0;
  answer = { status: "error", error: { code: "CANCELLED" } };
  await shell.onRepairGame("ghosting-renewed");
  answer = { status: "error", error: { code: "REPAIR_SOURCE_MISSING", details: { path: "bin/engine.dll" } } };
  await shell.onRepairGame("ghosting-renewed");
  assert.deepEqual(notes, ["#L_GM_REPAIR_FILE_UNAVAILABLE|bin/engine.dll"], "отмена молча, недоступный файл — по имени");
});

test("окно ремонта отвечает одним промисом, повторный вызов закрывает прежнее", async () => {
  const shell = instance({ data: { repairRequest: null } });
  shell.onRepairResolved = method("onRepairResolved").bind(shell);
  shell.askRepair = method("askRepair").bind(shell);
  const first = shell.askRepair([{ path: "a" }]);
  const second = shell.askRepair([{ path: "b" }]);
  assert.equal(await first, false);
  assert.deepEqual(shell.repairRequest.files, [{ path: "b" }]);
  shell.onRepairResolved(["b"]);
  assert.deepEqual(await second, ["b"]);
  assert.equal(shell.repairRequest, null);
  const menu = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameMenu.vue"), "utf8");
  assert.ok(menu.indexOf('id: "verify"') !== -1 && menu.indexOf('id: "verify"') < menu.indexOf('id: "repair"'), "«Проверить файлы» в меню перед «Восстановить игру»");
});
// Раунд 1 ревью: окно ремонта и его тексты.
test("размеры в окне ремонта — на языке окна: «24,7 МБ» / «24.7 MB»", () => {
  const src = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/file-format.js"), "utf8");
  const mod = { exports: {} };
  new Function("module", src.replace(/^export function /gm, "function ") + "\nmodule.exports = { formatSize };")(mod);
  const { formatSize } = mod.exports;
  const locale = (lang) => {
    const text = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    return (key) => JSON.parse(new RegExp(`"${key}": ("[^"]*")`).exec(text)[1]);
  };
  const ru = locale("ru"), en = locale("en");
  assert.equal(formatSize(25899827, ru), "24,7\u00a0МБ");
  assert.equal(formatSize(25899827, en), "24.7\u00a0MB");
  assert.equal(formatSize(398709, ru), "389,4\u00a0КБ");
  assert.equal(formatSize(16, ru), "16\u00a0Б");
  assert.equal(formatSize(5046547693, ru), "4,7\u00a0ГБ");
  for (const name of ["RepairModal", "GameDemos", "GameSettingsModal"]) {
    const vue = fs.readFileSync(path.join(__dirname, `../src/renderer/components/Launcher/${name}.vue`), "utf8");
    assert.match(vue, /formatSize\(bytes\) \{\s*return formatSize\(bytes, this\.\$t\);?\s*\}/, name);
  }
});

test("ошибки ремонта: файл из зеркала по имени, «обнови игру», отмена молча, остальное — общий текст", () => {
  const notes = [], generic = [];
  const shell = {
    $t: (key, ...args) => [key, ...args].join("|"),
    notify: (text) => notes.push(text),
    notifyDeliveryError: (result) => generic.push(result.error.code),
  };
  const notifyRepairError = method("notifyRepairError").bind(shell);
  const error = (code, details) => ({ status: "error", error: { code, details } });
  notifyRepairError(error("CANCELLED"));
  notifyRepairError(error("REPAIR_SOURCE_MISSING", { reason: "UPDATE_AVAILABLE" }));
  notifyRepairError(error("CHECKSUM_MISMATCH", { path: "bin/engine.dll" }));
  notifyRepairError(error("SIZE_MISMATCH", { path: "bin/engine.dll" }));
  notifyRepairError(error("REPAIR_SOURCE_MISSING", { path: "hl2.exe" }));
  notifyRepairError(error("NO_SPACE", { path: "hl2.exe" }));
  assert.deepEqual(notes, [
    "#L_GM_REPAIR_UPDATE_FIRST",
    "#L_GM_REPAIR_FILE_BROKEN|bin/engine.dll",
    "#L_GM_REPAIR_FILE_BROKEN|bin/engine.dll",
    "#L_GM_REPAIR_FILE_UNAVAILABLE|hl2.exe",
  ]);
  assert.deepEqual(generic, ["NO_SPACE"]);
  for (const lang of ["ru", "en"]) {
    const text = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    for (const key of ["#L_D_ERR_REPAIR_FAILED", "#L_D_ERR_SIZE_MISMATCH", "#L_D_ERR_ROLLBACK_FAILED", "#L_D_ERR_PATH_INVALID",
      "#L_GM_REPAIR_FILE_BROKEN", "#L_GM_REPAIR_UPDATE_FIRST", "#L_KB", "#L_B", "#L_DECIMAL"])
      assert.ok(text.includes(`"${key}":`), `${lang}: ${key}`);
    assert.doesNotMatch(/"#L_D_ERR_CHECKSUM_MISMATCH": "([^"]*)"/.exec(text)[1], /архив|archive/i);
  }
  const ru = fs.readFileSync(path.join(__dirname, "../src/renderer/utils/locales/ru.locale.js"), "utf8");
  for (const key of ["#L_D_ERR_ALREADY_RUNNING", "#L_D_ERR_GAME_RUNNING", "#L_GM_DISABLED_GAME_RUNNING", "#L_D_ERR_NETWORK_ERROR"])
    assert.doesNotMatch(new RegExp(`"${key}": "([^"]*)"`).exec(ru)[1], /(Дождитесь|Закройте|закройте|попробуйте)/, key);
});

test("«Проверить файлы»: разница в стоковых cfg — мягкая строка, не «Повреждено»; предложения с точкой", async () => {
  const ru = fs.readFileSync(path.join(__dirname, "../src/renderer/utils/locales/ru.locale.js"), "utf8");
  const $t = (key, ...args) => JSON.parse(new RegExp(`"${key}": ("[^"]*")`).exec(ru)[1]).replace(/%(\d)%/g, (all, n) => args[n]);
  const toast = async (result) => {
    const notes = [];
    const shell = { $t, notify: (text) => notes.push(text), runMenuAction: async () => result };
    shell.stockCfgParts = method("stockCfgParts").bind(shell);
    shell.stockCfgSentence = method("stockCfgSentence").bind(shell);
    await method("onVerifyFiles").call(shell, "ghosting-renewed");
    return notes[0];
  };
  assert.equal(await toast({ ok: true, damaged: 0, stockMissing: 2, configMissing: 2, configChanged: 2 }),
    "Файлы игры в порядке. Стоковые cfg: 2 нет, 2 отличаются — можно докачать в «Восстановить игру»");
  assert.equal(await toast({ ok: true, damaged: 0, stockMissing: 0, configMissing: 0, configChanged: 0 }), "Повреждений не обнаружено");
  // Нулевые части не выводятся.
  assert.equal(await toast({ ok: true, damaged: 0, stockMissing: 0, configMissing: 0, configChanged: 2 }),
    "Файлы игры в порядке. Стоковые cfg: 2 отличаются — можно докачать в «Восстановить игру»");
  assert.equal(await toast({ ok: false, damaged: 1, stockMissing: 2, configMissing: 2, configChanged: 0 }),
    "Повреждённых файлов: 1. Стоковые cfg: 2 нет.");
});

test("«Почему недоступно» не предлагает «Восстановить» играм без ремонта по файлам", () => {
  const repair = { id: "repair" }, verify = { id: "verify-files" }, diagnostics = { id: "open-diagnostics" }, discord = { id: "open-discord" };
  globalThis.ACTION = { repair, verify, diagnostics, discord };
  try {
    const actions = method("actions", undefined, undefined, "UnavailableModal");
    assert.deepEqual(actions.call({ fileRepair: true, view: { actions: [repair] } }), [repair]);
    assert.deepEqual(actions.call({ fileRepair: false, view: { actions: [repair] } }), [diagnostics, discord]);
    assert.deepEqual(actions.call({ fileRepair: false, view: { actions: [repair, diagnostics] } }), [diagnostics]);
  } finally {
    delete globalThis.ACTION;
  }
  const shell = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/LauncherShell.vue"), "utf8");
  assert.match(shell, /:fileRepair="!!\(currentDelivery && currentDelivery\.fileRepair\)"/);
});

test("полоса загрузки: меньше мегабайта всего — в КБ, а не «0 МБ / 0 МБ»", () => {
  const bar = { progressTotalBytes: 30 * 1024, $t: (key) => key };
  bar.sizeText = method("sizeText", undefined, undefined, "DownloadBar").bind(bar);
  const bytesText = method("bytesText", undefined, undefined, "DownloadBar").bind(bar);
  assert.equal(bytesText(0), "0 #L_KB");
  assert.equal(bytesText(30 * 1024), "30 #L_KB");
  bar.progressTotalBytes = 300 * 1024 * 1024;
  assert.equal(bytesText(150 * 1024 * 1024), "150 #L_MB");
});
// Раунд 2 ревью: размеры в окне удаления, тостах экспорта и полосе загрузки — по языку.
test("formatBytes и полоса загрузки: размер по языку окна", () => {
  const src = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/file-format.js"), "utf8");
  const mod = { exports: {} };
  new Function("module", src.replace(/^export function /gm, "function ") + "\nmodule.exports = { formatSize, formatNumber: typeof formatNumber === 'function' ? formatNumber : undefined };")(mod);
  const locale = (lang) => {
    const text = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    return (key) => JSON.parse(new RegExp(`"${key}": ("[^"]*")`).exec(text)[1]);
  };
  const ru = locale("ru"), en = locale("en");
  Object.assign(globalThis, mod.exports);
  try {
    const sizeText = method("sizeText", undefined, undefined, "DownloadBar");
    assert.equal(sizeText.call({ $t: en }, 1230), "1.23 GB");
    assert.equal(sizeText.call({ $t: ru }, 1230), "1,23 ГБ");
    const formatBytes = method("formatBytes");
    assert.equal(formatBytes.call({ $t: ru }, 25899827), "24,7\u00a0МБ");
    assert.equal(formatBytes.call({ $t: en }, 25899827), "24.7\u00a0MB");
    assert.equal(formatBytes.call({ $t: ru }, 0), "", "неизвестный размер — пусто, как раньше");
  } finally {
    delete globalThis.formatSize;
    delete globalThis.formatNumber;
  }
});
// Раунд 3 ревью.
test("итог ремонта: снятый пропавший стоковый cfg — мягкая строка, без «Осталось» и «Проверить снова»", async () => {
  const notes = [], asked = [];
  const ipc = { invoke: async (channel, id, options) => options
    ? { status: "repaired", repaired: ["bin/engine.dll"], remaining: [], integrity: { ok: true, stockMissing: 1, configChanged: 0 } }
    : { status: "confirmation-required", damaged: 1, files: [{ path: "bin/engine.dll", size: 1, kind: "missing" }] } };
  const shell = {
    $t: (key, ...args) => [key, ...args].join("|"),
    notify: (text) => notes.push(text),
    askRepair: async () => ["bin/engine.dll"],
    askConfirm: async (request) => { asked.push(request); return false; },
  };
  shell.notifyRepairError = method("notifyRepairError").bind(shell);
  shell.stockCfgParts = method("stockCfgParts").bind(shell);
  shell.stockCfgSentence = method("stockCfgSentence").bind(shell);
  await method("onRepairGame", undefined, ipc).call(shell, "ghosting-renewed");
  assert.deepEqual(asked, []);
  assert.deepEqual(notes, ["#L_GM_REPAIR_DONE_N|1. #L_GM_CFG_DIFF_N|#L_GM_CFG_MISSING_N|1."]);
  for (const [lang, text] of [["ru", "Отмеченные стоковые cfg заменят твои — старые удалятся"], ["en", "Ticked stock cfg files will replace yours, and the old ones will be deleted"]]) {
    const locale = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    assert.ok(locale.includes(`"#L_GM_REPAIR_CFG_WARN": "${text}"`), lang);
  }
});

test("скорость и размеры по языку окна: полоса загрузки, обновление лаунчера, размер инструмента", () => {
  const src = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/file-format.js"), "utf8");
  const mod = { exports: {} };
  new Function("module", src.replace(/^export function /gm, "function ") + "\nmodule.exports = { formatSize, formatNumber: typeof formatNumber === 'function' ? formatNumber : undefined };")(mod);
  const locale = (lang) => {
    const text = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    return (key, ...args) => JSON.parse(new RegExp(`"${key}": ("[^"]*")`).exec(text)[1]).replace(/%(\d)%/g, (all, n) => args[n]);
  };
  const ru = locale("ru"), en = locale("en");
  Object.assign(globalThis, mod.exports);
  try {
    const speed = method("speedText", undefined, undefined, "DownloadBar");
    assert.equal(speed.call({ $t: en, delivery: { speedBps: 1.5 * 1048576 } }), en("#L_D_SPEED", "1.5"));
    assert.equal(speed.call({ $t: ru, delivery: { speedBps: 1.5 * 1048576 } }), ru("#L_D_SPEED", "1,5"));
    const progress = { transferred: 12.3 * 1048576, total: 45.6 * 1048576, bytesPerSecond: 1.5 * 1048576 };
    const loadingBytes = method("progressBytes", undefined, undefined, "../../views/Loading");
    const loadingSpeed = method("progressSpeed", undefined, undefined, "../../views/Loading");
    assert.equal(loadingBytes.call({ $t: ru, progress }), "12,3\u00a0МБ / 45,6\u00a0МБ");
    assert.equal(loadingBytes.call({ $t: en, progress }), "12.3\u00a0MB / 45.6\u00a0MB");
    assert.equal(loadingSpeed.call({ $t: ru, progress }), ru("#L_U_SPEED", "1,5"));
    const toolSize = method("sizeText", undefined, undefined, "ToolPanel");
    assert.equal(toolSize.call({ $t: ru, tool: { sizeBytes: 12.3 * 1048576 } }), "12,3\u00a0МБ");
    assert.equal(toolSize.call({ $t: en, tool: { sizeBytes: 12.3 * 1048576 } }), "12.3\u00a0MB");
  } finally {
    delete globalThis.formatSize;
    delete globalThis.formatNumber;
  }
});
// Достижения 0.4.9: события интерфейса, меню SteamID и 100 % в профиле.
test("«Франкенштейн»: «Скопировать код» во вкладке «Код настроек» открывает достижение, неудачный экспорт — нет", async () => {
  const calls = [];
  let exported = { status: "ok", code: "HLSR1-abc" };
  const ipc = { invoke: async (channel, ...args) => { calls.push([channel, ...args]); return channel === "game-personalization-code-export" ? exported : {}; } };
  globalThis.clipboard = { writeText: async () => ({ status: "ok" }) };
  try {
    const panel = () => {
      const vm = instance({ data: { gameId: "218", look: {}, busy: "", disabled: false, invalid: false, copied: false, exportCode: "", exportError: "" },
        methods: { copyCode: method("copyCode", undefined, ipc, "GamePersonalizationCode") } });
      vm.alive = true; vm.exportRevision = 0; vm.$t = (key) => key;
      return vm;
    };
    exported = { status: "error" };
    await panel().copyCode();
    assert.equal(calls.some(([channel]) => channel === "runner-secret-unlock"), false);
    exported = { status: "ok", code: "HLSR1-abc" };
    const vm = panel();
    await vm.copyCode();
    assert.equal(vm.copied, true);
    assert.deepEqual(calls.filter(([channel]) => channel === "runner-secret-unlock"), [["runner-secret-unlock", "frankenstein"]]);
  } finally {
    delete globalThis.clipboard;
  }
});

test("«Синдром Плюшкина»: в «Уведомлениях» накопилось 50", (t) => {
  const calls = [];
  const center = notificationCenter(t, { invoke: async (channel, id) => { calls.push([channel, id]); return { items: [], unread: 0 }; } });
  const items = (count) => Array.from({ length: count }, (_, i) => ({ id: "n" + i, code: "X", level: "info", read: true, ts: 1 }));
  center.applySnapshot({ items: items(49), unread: 0 });
  assert.deepEqual(calls, []);
  center.applySnapshot({ items: items(50), unread: 0 });
  assert.deepEqual(calls, [["runner-secret-unlock", "hoarder"]]);
});

test("«Пианино»: только сохранённые бинды, все клавиши полной клавиатуры, проверка и при открытии", () => {
  const calls = [];
  const ipc = { invoke: async (channel, id) => { calls.push([channel, id]); } };
  const kb = (name) => method(name, undefined, ipc, "GameKeyboard");
  const bound = { command: "+jump" };
  const vm = instance({ data: { savedValue: {}, fullKeys: [{ key: "a" }, { key: "b" }], savedEffective: { a: bound }, effective: { a: bound, b: bound } },
    computed: { everyKeyBound: kb("everyKeyBound") }, methods: { unlockPiano: kb("unlockPiano") } });
  assert.equal(vm.everyKeyBound, false, "занятый черновик без сохранения не считается");
  vm.savedEffective = { a: bound, b: bound };
  assert.equal(vm.everyKeyBound, true);
  vm.savedValue = null;
  assert.equal(vm.everyKeyBound, false, "сохранённое ещё не загружено");
  vm.savedValue = {}; vm.fullKeys = [];
  assert.equal(vm.everyKeyBound, false);
  vm.unlockPiano(false);
  vm.unlockPiano(true);
  assert.deepEqual(calls, [["runner-secret-unlock", "piano"]]);

  // Клавиши — полной раскладки, даже когда на экране компактная.
  const src = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/keyboard-layout.js"), "utf8");
  const mod = { exports: {} };
  new Function("module", src.replace(/^export \{([^}]*)\};?\s*$/m, "module.exports = {$1};"))(mod);
  globalThis.layoutFor = mod.exports.layoutFor;
  try {
    const keys = kb("fullKeys").call({ catalog: { engine: "goldsrc" }, full: false });
    const live = (layout) => layout.keys.filter((cap) => cap.key).length;
    assert.equal(keys.length, live(mod.exports.layoutFor("goldsrc", { full: true })));
    assert.ok(keys.length > live(mod.exports.layoutFor("goldsrc", { full: false })));
    assert.ok(keys.every((cap) => cap.key));
  } finally {
    delete globalThis.layoutFor;
  }

  const vue = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameKeyboard.vue"), "utf8");
  assert.match(vue, /everyKeyBound: \{ handler: "unlockPiano", immediate: true \}/, "проверка и при открытии клавиатуры");
  // Окно настроек отдаёт клавиатуре сохранённое: бинды последнего сохранения и cfg с диска (без импорта в черновике).
  const modal = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/GameSettingsModal.vue"), "utf8");
  assert.equal((modal.match(/<GameKeyboard[^>]*:saved-value="savedBinds"[^>]*:saved-key-binds="keyBinds"/g) || []).length, 2);
  const savedBinds = method("savedBinds", undefined, ipc, "GameSettingsModal");
  assert.deepEqual(savedBinds.call({ draftSaved: JSON.stringify({ binds: { a: { command: "+jump", base: "" } } }) }), { a: { command: "+jump", base: "" } });
  assert.deepEqual(savedBinds.call({ draftSaved: JSON.stringify({}) }), {});
  assert.equal(savedBinds.call({ draftSaved: null }), null);
});

test("«Пианино»: заняты и все кнопки мыши со схемы — MOUSE1–5, колесо вверх и вниз", () => {
  const kb = (name) => method(name, undefined, { invoke: async () => {} }, "GameKeyboard");
  const src = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/keyboard-layout.js"), "utf8");
  const mod = { exports: {} };
  new Function("module", src.replace(/^export \{([^}]*)\};?\s*$/m, "module.exports = {$1};"))(mod);
  globalThis.layoutFor = mod.exports.layoutFor;
  try {
    const mouse = ["MOUSE1", "MOUSE2", "MOUSE3", "MOUSE4", "MOUSE5", "MWHEELUP", "MWHEELDOWN"];
    const bound = (keys) => Object.fromEntries(keys.map((key) => [key, { command: "+jump" }]));
    for (const engine of ["goldsrc", "source"]) {
      const fullKeys = kb("fullKeys").call({ catalog: { engine }, full: false });
      const keys = fullKeys.map((cap) => cap.key);
      const vm = instance({ data: { savedValue: {}, fullKeys, savedEffective: bound(keys) }, computed: { everyKeyBound: kb("everyKeyBound") } });
      assert.equal(vm.everyKeyBound, true, `${engine}: заняты все клавиши и кнопки мыши`);
      for (const key of mouse) {
        vm.savedEffective = bound(keys.filter((other) => other !== key));
        assert.equal(vm.everyKeyBound, false, `${engine}: свободна ${key}`);
      }
    }
  } finally {
    delete globalThis.layoutFor;
  }
});

test("профиль: SteamID не в шапке, меню «⋯» — копия и Steam, у скрытого SteamID пункта Steam нет", () => {
  const items = method("moreItems", undefined, undefined, "ProfileModal");
  const other = "76561198000000222";
  const run = (props) => items.call(Object.assign({ targetSteamId: "", hideSteamId: false, details: {}, account: { steamId2: "STEAM_0:1:1", steamId64: "76561197960265731" } }, props)).map((item) => item.id);
  assert.deepEqual(run({}), ["copy", "steam"]);
  assert.deepEqual(run({ hideSteamId: true }), ["copy"], "свой скрытый: копировать можно, ссылки нет");
  assert.deepEqual(run({ targetSteamId: other, details: { hideSteamId: false } }), ["copy", "steam"]);
  assert.deepEqual(run({ targetSteamId: other, details: { hideSteamId: true } }), [], "чужой скрытый: ни номера, ни ссылки");
  const vue = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/ProfileModal.vue"), "utf8");
  assert.doesNotMatch(vue, /profile__id-value|idText/, "номера в шапке больше нет");
  assert.match(vue, /aria-haspopup="menu"/);
  assert.match(vue, /role="menuitem"/);
});

test("профиль: 100 % — золотая рамка по часовой, редкость на карточках, без анимаций всё стоит", () => {
  const complete = method("complete", undefined, undefined, "ProfileModal");
  assert.equal(complete.call({ summary: { unlocked: 37, total: 37 } }), true);
  assert.equal(complete.call({ summary: { unlocked: 36, total: 37 } }), false);
  assert.equal(complete.call({ summary: { unlocked: 0, total: 0 } }), false);
  const vue = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/ProfileModal.vue"), "utf8");
  assert.match(vue, /'is-complete': complete/);
  assert.match(vue, /<span v-if="\(subscriber \|\| complete\) && !guestWindow" class="neon"/);
  for (const tier of ["gold", "purple", "rainbow"]) assert.ok(vue.includes(".tier-" + tier), tier);
  assert.ok(vue.includes(".launcher-root.no-anim .profile__achievement.tier-rainbow"), "переключатель анимаций лаунчера");
  assert.match(vue, /@media \(prefers-reduced-motion: reduce\) \{[^}]*tier-rainbow/);
});

// \u041f\u0430\u0441\u0445\u0430\u043b\u043a\u0438 0.4.9: \u00ab\u043f\u043e\u0432\u0442\u043e\u0440\u0438 \u0446\u0432\u0435\u0442\u0430\u00bb \u043d\u0430 \u0448\u0430\u0440\u0438\u043a\u0430\u0445 LiveSplit, HESOYAM, \u043f\u043e\u043d\u0438 \u0443 \u043b\u043e\u0433\u043e\u0442\u0438\u043f\u0430.
const memoryGame = require("../src/renderer/components/Launcher/memory-game");

// \u041f\u0440\u043e\u0445\u043e\u0434\u0438\u0442 \u0438\u0433\u0440\u0443 \u0431\u0435\u0437 \u043e\u0448\u0438\u0431\u043e\u043a, \u043d\u0430\u0436\u0438\u043c\u0430\u044f \u043a\u0430\u0436\u0434\u044b\u0439 \u0440\u0430\u0443\u043d\u0434 \u0432\u0441\u044e \u043f\u043e\u0441\u043b\u0435\u0434\u043e\u0432\u0430\u0442\u0435\u043b\u044c\u043d\u043e\u0441\u0442\u044c.
function playMemoryRounds(press, game, rounds) {
  for (let round = 0; round < rounds; round++) {
    const sequence = game().sequence;
    sequence.forEach((orb) => press(orb));
  }
}

test("\u043f\u043e\u0432\u0442\u043e\u0440\u0438 \u0446\u0432\u0435\u0442\u0430: \u0440\u0430\u0443\u043d\u0434 1 \u2014 \u043e\u0434\u0438\u043d \u0446\u0432\u0435\u0442, \u0440\u0430\u0443\u043d\u0434 \u0434\u043e\u0431\u0430\u0432\u043b\u044f\u0435\u0442 \u043f\u043e \u0441\u043b\u0443\u0447\u0430\u0439\u043d\u043e\u043c\u0443, \u043e\u0448\u0438\u0431\u043a\u0430 \u2014 \u0441\u0431\u0440\u043e\u0441, 6-\u0439 \u0440\u0430\u0443\u043d\u0434 \u2014 \u043f\u043e\u0431\u0435\u0434\u0430", () => {
  assert.equal(memoryGame.ROUNDS, 8, "восемь раундов (\u043f\u0440\u043e\u0441\u044c\u0431\u0430 \u0432\u043b\u0430\u0434\u0435\u043b\u044c\u0446\u0430 11.11)");
  assert.deepEqual(memoryGame.start(3, () => 0), { sequence: [0], step: 0 });
  assert.deepEqual(memoryGame.start(3, () => 0.9999), { sequence: [2], step: 0 });
  const seen = new Set();
  for (let i = 0; i < 200; i++) seen.add(memoryGame.start(3).sequence[0]);
  assert.deepEqual([...seen].sort(), [0, 1, 2], "\u0446\u0432\u0435\u0442\u0430 \u0441\u043b\u0443\u0447\u0430\u0439\u043d\u044b\u0435 \u0438 \u0432\u0441\u0435 \u0442\u0440\u0438");

  let game = { sequence: [1, 0], step: 0 };
  let move = memoryGame.press(game, 1, 3, () => 0.5);
  assert.deepEqual(move, { game: { sequence: [1, 0], step: 1 }, result: "ok" });
  move = memoryGame.press(move.game, 0, 3, () => 0.5);
  assert.deepEqual(move, { game: { sequence: [1, 0, 1], step: 0 }, result: "round" }, "\u043d\u043e\u0432\u044b\u0439 \u0440\u0430\u0443\u043d\u0434 \u2014 \u0442\u043e\u0442 \u0436\u0435 \u043f\u043e\u0440\u044f\u0434\u043e\u043a \u043f\u043b\u044e\u0441 \u043e\u0434\u0438\u043d \u0446\u0432\u0435\u0442");
  assert.deepEqual(memoryGame.press(move.game, 2, 3), { game: null, result: "wrong" });

  const results = [];
  game = memoryGame.start(3);
  for (;;) {
    move = memoryGame.press(game, game.sequence[game.step], 3);
    results.push(move.result);
    if (!move.game) break;
    game = move.game;
  }
  assert.equal(results[results.length - 1], "won");
  assert.equal(results.filter((result) => result === "round").length, memoryGame.ROUNDS - 1, "победа после 8-\u0433\u043e \u0440\u0430\u0443\u043d\u0434\u0430");
  assert.equal(results.length, memoryGame.ROUNDS * (memoryGame.ROUNDS + 1) / 2);
});

test("повтори цвета на шариках LiveSplit: любой щелчок начинает (вспыхивают все три), ошибка гасит шарики, 5 с без щелчков — новая игра, 8 раундов открывают good-memory", () => {
  const invoked = [];
  const ipc = { invoke: (...args) => { invoked.push(args); return Promise.resolve(); } };
  const ALL_ORBS = 3;
  Object.assign(globalThis, { memoryGame, ORB_COUNT: 3, ALL_ORBS, MEMORY_IDLE: 5000, MEMORY_MISS: 700 });
  const egg = { memory: null, memoryPlaying: false, memoryMiss: false, memoryLast: 0, memoryTimers: [], litOrb: -1, litTimer: null };
  try {
    let plays = 0;
    const flashes = [];
    // Как настоящий playMemory: после показа отсчёт тишины идёт заново.
    Object.assign(egg, { playMemory() { plays += 1; this.memoryLast = performance.now(); }, flashOrb: (orb) => flashes.push(orb) });
    const click = method("onOrbClick", undefined, ipc, "LauncherBackground").bind(egg);

    click(2);
    assert.equal(egg.memory.sequence.length, 1, "первый щелчок начинает раунд 1");
    assert.equal(plays, 1, "и показывает первую подсветку");
    assert.deepEqual(flashes, [ALL_ORBS], "старт видно: вспыхивают все три");

    egg.memoryPlaying = true;
    click(egg.memory.sequence[0]);
    assert.equal(egg.memory.step, 0, "пока идёт показ, щелчки не считаются");
    egg.memoryPlaying = false;

    click((egg.memory.sequence[0] + 1) % 3);
    assert.equal(egg.memory, null, "ошибка — сброс");
    assert.equal(egg.memoryMiss, true, "шарики гаснут");
    assert.deepEqual(flashes, [ALL_ORBS], "ошибка шарик не подсвечивает");
    assert.deepEqual(invoked, []);

    click(0);
    assert.deepEqual(flashes, [ALL_ORBS, ALL_ORBS], "новая игра — снова все три");
    assert.equal(egg.memoryMiss, false);
    egg.memoryLast = performance.now() - 6000;
    click(egg.memory.sequence[0]);
    assert.equal(egg.memory.step, 0, "после 5 с тишины щелчок начинает новую игру, а не отвечает");
    assert.equal(plays, 3);

    const before = flashes.length;
    playMemoryRounds(click, () => egg.memory, 7);
    assert.equal(egg.memory.sequence.length, 8);
    assert.equal(flashes.length - before, 1 + 2 + 3 + 4 + 5 + 6 + 7, "каждый свой щелчок подсвечивает шарик");
    assert.deepEqual(invoked, [], "семь раундов — ещё не победа");
    playMemoryRounds(click, () => egg.memory, 1);
    assert.deepEqual(invoked, [["runner-secret-unlock", "good-memory"]]);
    assert.equal(egg.memory, null, "после победы игра начинается заново");
    assert.equal(plays, 3 + 7, "показ перед каждым раундом");
  } finally {
    egg.memoryTimers.forEach(clearTimeout);
    for (const name of ["memoryGame", "ORB_COUNT", "ALL_ORBS", "MEMORY_IDLE", "MEMORY_MISS"]) delete globalThis[name];
  }
  // \u0418\u0433\u0440\u0430 \u0442\u0430\u0439\u043d\u0430\u044f: \u0443 \u0448\u0430\u0440\u0438\u043a\u043e\u0432 \u043d\u0435\u0442 \u043f\u043e\u0434\u0441\u043a\u0430\u0437\u043e\u043a \u0438 \u0441\u0432\u043e\u0435\u0433\u043e \u043a\u0443\u0440\u0441\u043e\u0440\u0430.
  const bg = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/LauncherBackground.vue"), "utf8");
  const orbs = bg.slice(bg.indexOf('class="launcher-bg__tool"'), bg.indexOf("</transition>", bg.indexOf('class="launcher-bg__tool"')));
  assert.doesNotMatch(orbs, /title|aria-label|tabindex/);
  assert.doesNotMatch(bg.slice(bg.indexOf("<style")), /launcher-bg__orb[^{]*\{[^}]*cursor/);
  assert.match(bg.slice(bg.indexOf("<style")), /\.launcher-bg__orb \{[^}]*pointer-events: auto;/, "шарики ловят щелчки сквозь прозрачный слой фона");
  assert.match(fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/LauncherShell.vue"), "utf8"),
    /\.launcher-main--tool \{\s*pointer-events: none;\s*\}\s*\.launcher-main--tool \.launcher-dock \{\s*pointer-events: auto;/,
    "\u0449\u0435\u043b\u0447\u043a\u0438 \u0434\u043e\u0445\u043e\u0434\u044f\u0442 \u0434\u043e \u0448\u0430\u0440\u0438\u043a\u043e\u0432 \u0441\u043a\u0432\u043e\u0437\u044c \u0441\u0446\u0435\u043d\u0443, \u043d\u0438\u0436\u043d\u044f\u044f \u043f\u0430\u043d\u0435\u043b\u044c LiveSplit \u043d\u0430\u0436\u0438\u043c\u0430\u0435\u0442\u0441\u044f");
});

test("\u043f\u043e\u0432\u0442\u043e\u0440\u0438 \u0446\u0432\u0435\u0442\u0430: \u0443\u0445\u043e\u0434 \u0441\u043e \u0441\u0442\u0440\u0430\u043d\u0438\u0446\u044b LiveSplit \u0433\u0430\u0441\u0438\u0442 \u043f\u043e\u043a\u0430\u0437 \u0438 \u0441\u0431\u0440\u0430\u0441\u044b\u0432\u0430\u0435\u0442 \u0438\u0433\u0440\u0443", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  Object.assign(globalThis, { MEMORY_PAUSE: 700, MEMORY_STEP: 600, MEMORY_LIT: 400 });
  try {
    const bg = { memory: { sequence: [0, 2, 1], step: 0 }, memoryPlaying: false, memoryTimers: [], litOrb: -1 };
    for (const name of ["playMemory", "flashOrb", "resetMemory"]) bg[name] = method(name, undefined, undefined, "LauncherBackground").bind(bg);
    const watcher = method("toolBackground", undefined, undefined, "LauncherBackground").bind(bg);
    bg.playMemory();
    t.mock.timers.tick(800);
    assert.equal(bg.litOrb, 0, "\u043f\u043e\u043a\u0430\u0437 \u0438\u0434\u0451\u0442");
    watcher(true);
    assert.equal(bg.litOrb, 0, "\u043f\u043e\u043a\u0430 \u0441\u0442\u0440\u0430\u043d\u0438\u0446\u0430 \u043e\u0442\u043a\u0440\u044b\u0442\u0430, \u0438\u0433\u0440\u0430 \u0438\u0434\u0451\u0442");
    watcher(false);
    assert.deepEqual([bg.memory, bg.memoryPlaying, bg.litOrb, bg.memoryTimers.length], [null, false, -1, 0]);
    t.mock.timers.tick(5000);
    assert.equal(bg.litOrb, -1, "\u043e\u0442\u043b\u043e\u0436\u0435\u043d\u043d\u044b\u0435 \u043f\u043e\u0434\u0441\u0432\u0435\u0442\u043a\u0438 \u043d\u0435 \u0441\u0440\u0430\u0431\u0430\u0442\u044b\u0432\u0430\u044e\u0442 \u043f\u043e\u0441\u043b\u0435 \u0443\u0445\u043e\u0434\u0430");
    assert.equal(bg.memoryPlaying, false);
  } finally {
    delete globalThis.MEMORY_PAUSE;
    delete globalThis.MEMORY_STEP;
    delete globalThis.MEMORY_LIT;
  }
});

test("\u0437\u0432\u0443\u043a\u0438 \u0438 \u0432\u0438\u0434\u0435\u043e renderer \u2014 \u0432\u0441\u0435\u0433\u0434\u0430 \u043e\u0442\u0434\u0435\u043b\u044c\u043d\u044b\u043c \u0444\u0430\u0439\u043b\u043e\u043c: CSP \u043d\u0435 \u043f\u0443\u0441\u043a\u0430\u0435\u0442 data: \u0432 media-src", () => {
  const config = require("../.electron-vue/webpack.renderer.config.js");
  for (const file of ["gta-ui.mp3", "clip.mp4", "beep.wav", "a.ogg"]) {
    const rule = config.module.rules.find((item) => item.test instanceof RegExp && item.test.test(file));
    assert.ok(rule, file);
    assert.equal(rule.type, "asset/resource", file);
  }
});

test("HESOYAM \u043f\u043e \u0444\u0438\u0437\u0438\u0447\u0435\u0441\u043a\u0438\u043c \u043a\u043b\u0430\u0432\u0438\u0448\u0430\u043c: \u043f\u043b\u0430\u0448\u043a\u0430 \u0432 \u0433\u043b\u0430\u0432\u043d\u043e\u043c \u043e\u043a\u043d\u0435, \u043d\u0435 \u0432 \u043f\u043e\u043b\u044f\u0445 \u0432\u0432\u043e\u0434\u0430 \u0438 \u043d\u0435 \u043f\u043e\u0434 \u043e\u043a\u043d\u0430\u043c\u0438", () => {
  Object.assign(globalThis, {
    SECRET_SEQUENCE: ["ArrowUp", "ArrowUp", "ArrowDown", "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowLeft", "ArrowRight", "KeyB", "KeyA"],
    CHEAT_SEQUENCE: ["KeyH", "KeyE", "KeyS", "KeyO", "KeyY", "KeyA", "KeyM"],
  });
  try {
    const shellSource = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/LauncherShell.vue"), "utf8");
    assert.ok(shellSource.includes('const CHEAT_SEQUENCE = ["KeyH", "KeyE", "KeyS", "KeyO", "KeyY", "KeyA", "KeyM"];'));
    const keydown = method("onGlobalKeydown");
    const shell = () => {
      const vm = { overlayOpen: false, keySequence: [], cheats: 0, unlocked: [] };
      vm.showCheat = () => { vm.cheats += 1; };
      vm.unlockAchievement = (id) => vm.unlocked.push(id);
      vm.type = (codes, target = { tagName: "DIV" }) => codes.forEach((code) => keydown.call(vm, { code, key: "\u0440", target }));
      return vm;
    };
    const hesoyam = globalThis.CHEAT_SEQUENCE;

    let vm = shell();
    vm.type(["KeyQ", "Space", ...hesoyam]);
    assert.equal(vm.cheats, 1, "\u0440\u0443\u0441\u0441\u043a\u0430\u044f \u0440\u0430\u0441\u043a\u043b\u0430\u0434\u043a\u0430: event.key \u00ab\u0440\u00bb, event.code KeyH");
    vm.type(hesoyam);
    assert.equal(vm.cheats, 2, "\u043f\u043e\u0432\u0442\u043e\u0440\u043d\u044b\u0439 \u0432\u0432\u043e\u0434 \u0441\u043d\u043e\u0432\u0430 \u043f\u043e\u043a\u0430\u0437\u044b\u0432\u0430\u0435\u0442 \u043f\u043b\u0430\u0448\u043a\u0443");
    vm.type(hesoyam.slice(0, 6));
    assert.equal(vm.cheats, 2);

    vm = shell();
    vm.type(hesoyam, { tagName: "INPUT" });
    vm.type(hesoyam, { tagName: "TEXTAREA" });
    vm.type(hesoyam, { tagName: "DIV", isContentEditable: true });
    assert.equal(vm.cheats, 0, "\u0432 \u043f\u043e\u043b\u044f\u0445 \u0432\u0432\u043e\u0434\u0430 \u043d\u0435 \u0441\u0440\u0430\u0431\u0430\u0442\u044b\u0432\u0430\u0435\u0442");
    vm.overlayOpen = true;
    vm.type(hesoyam);
    assert.equal(vm.cheats, 0, "\u043f\u043e\u0434 \u043e\u0442\u043a\u0440\u044b\u0442\u044b\u043c \u043e\u043a\u043d\u043e\u043c \u043d\u0435 \u0441\u0440\u0430\u0431\u0430\u0442\u044b\u0432\u0430\u0435\u0442");
    vm.overlayOpen = false;
    // \u041e\u043a\u043d\u0430, \u043a\u043e\u0442\u043e\u0440\u044b\u0445 \u043d\u0435\u0442 \u0432 overlayOpen: \u0432\u044b\u0431\u043e\u0440 \u0438\u0433\u0440\u044b LiveSplit, \u043e\u043f\u0440\u043e\u0441 \u043a\u043e\u043d\u0444\u0438\u0433\u0430, \u043f\u0430\u0434\u0435\u043d\u0438\u0435 \u0438\u0433\u0440\u044b.
    for (const modal of ["liveSplitLaunch", "configSetup", "crashReport"]) {
      vm[modal] = {};
      vm.type(hesoyam);
      vm.type(globalThis.SECRET_SEQUENCE);
      assert.equal(vm.cheats, 0, `\u043f\u043e\u0434 ${modal} HESOYAM \u043d\u0435 \u0441\u0440\u0430\u0431\u0430\u0442\u044b\u0432\u0430\u0435\u0442`);
      assert.deepEqual(vm.unlocked, [], `\u043f\u043e\u0434 ${modal} Konami \u043d\u0435 \u0441\u0440\u0430\u0431\u0430\u0442\u044b\u0432\u0430\u0435\u0442`);
      vm[modal] = null;
    }
    vm.type(globalThis.SECRET_SEQUENCE);
    assert.deepEqual(vm.unlocked, ["wake-up"], "Konami \u0440\u0430\u0431\u043e\u0442\u0430\u0435\u0442 \u043a\u0430\u043a \u0440\u0430\u043d\u044c\u0448\u0435");
  } finally {
    delete globalThis.SECRET_SEQUENCE;
    delete globalThis.CHEAT_SEQUENCE;
  }
});

test("HESOYAM: \u043f\u043b\u0430\u0448\u043a\u0430 \u0441\u043e \u0437\u0432\u0443\u043a\u043e\u043c \u0434\u0435\u0440\u0436\u0438\u0442\u0441\u044f 2,6 \u0441, \u0434\u043e\u0441\u0442\u0438\u0436\u0435\u043d\u0438\u0435 hesoyam; \u043f\u043e\u0432\u0442\u043e\u0440 \u2014 \u0441\u043d\u043e\u0432\u0430 \u043f\u043b\u0430\u0448\u043a\u0430 \u0438 \u0437\u0432\u0443\u043a", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const sounds = [];
  globalThis.CHEAT_SOUND = "gta-ui.mp3";
  globalThis.Audio = class {
    constructor(src) { this.src = src; sounds.push(this); this.plays = 0; }
    play() { this.plays += 1; this.playedAt = { volume: this.volume, currentTime: this.currentTime }; return Promise.resolve(); }
  };
  try {
    const unlocked = [];
    const shell = { cheatShown: false, unlockAchievement: (id) => unlocked.push(id) };
    const showCheat = method("showCheat").bind(shell);
    showCheat();
    assert.equal(shell.cheatShown, true);
    assert.equal(sounds.length, 1);
    assert.deepEqual(sounds[0].playedAt, { volume: 0.35, currentTime: 0 });
    assert.deepEqual(unlocked, ["hesoyam"]);
    t.mock.timers.tick(2000);
    sounds[0].currentTime = 0.2;
    showCheat();
    assert.equal(sounds.length, 1, "\u0437\u0432\u0443\u043a \u043e\u0434\u0438\u043d \u0438 \u0442\u043e\u0442 \u0436\u0435");
    assert.equal(sounds[0].plays, 2);
    assert.equal(sounds[0].playedAt.currentTime, 0, "\u0437\u0432\u0443\u043a \u0441 \u043d\u0430\u0447\u0430\u043b\u0430");
    t.mock.timers.tick(2000);
    assert.equal(shell.cheatShown, true, "\u043f\u043e\u0432\u0442\u043e\u0440 \u043f\u0440\u043e\u0434\u043b\u0435\u0432\u0430\u0435\u0442 \u043f\u043b\u0430\u0448\u043a\u0443");
    t.mock.timers.tick(600);
    assert.equal(shell.cheatShown, false);
  } finally {
    delete globalThis.CHEAT_SOUND;
    delete globalThis.Audio;
  }
  for (const [lang, text] of [["ru", "\u0427\u0438\u0442 \u0430\u043a\u0442\u0438\u0432\u0438\u0440\u043e\u0432\u0430\u043d"], ["en", "Cheat activated"]]) {
    const locale = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    assert.ok(locale.includes(`"#L_CHEAT_ACTIVATED": "${text}"`), lang);
  }
});

test("\u043f\u043e\u043d\u0438 \u0438 \u00ab\u041d\u0430\u0437\u0430\u0434 \u0432 \u043f\u0440\u043e\u0448\u043b\u043e\u0435!\u00bb \u2014 \u043f\u043e \u0442\u0440\u0451\u043c \u0431\u044b\u0441\u0442\u0440\u044b\u043c \u0449\u0435\u043b\u0447\u043a\u0430\u043c \u043f\u043e \u043b\u043e\u0433\u043e\u0442\u0438\u043f\u0443; \u0434\u0432\u0430 \u0449\u0435\u043b\u0447\u043a\u0430 \u0438\u043b\u0438 \u043c\u0435\u0434\u043b\u0435\u043d\u043d\u044b\u0435 \u0442\u0440\u0438 \u2014 \u043d\u0435\u0442", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const unlocked = [];
  const shell = { logoColor: "", ponyShown: false, unlockAchievement: (id) => unlocked.push(id) };
  shell.unlockBackToPast = method("unlockBackToPast").bind(shell);
  shell.showPony = method("showPony").bind(shell);
  const click = method("onLogoClick").bind(shell);
  const doubleClick = method("onLogoDoubleClick").bind(shell);
  const clicks = (...times) => times.forEach((timeStamp) => click({ timeStamp }));
  // \u041e\u0431\u044b\u0447\u043d\u044b\u0439 \u0434\u0432\u043e\u0439\u043d\u043e\u0439 \u0449\u0435\u043b\u0447\u043e\u043a \u0442\u043e\u043b\u044c\u043a\u043e \u043f\u0435\u0440\u0435\u043a\u0440\u0430\u0448\u0438\u0432\u0430\u0435\u0442, \u0434\u0430\u0436\u0435 \u0435\u0441\u043b\u0438 \u043f\u043e\u0432\u0442\u043e\u0440\u0438\u0442\u044c \u0435\u0433\u043e \u0447\u0435\u0440\u0435\u0437 \u0441\u0435\u043a\u0443\u043d\u0434\u0443.
  clicks(1000, 1150); doubleClick();
  assert.notEqual(shell.logoColor, "", "\u0434\u0432\u043e\u0439\u043d\u043e\u0439 \u0449\u0435\u043b\u0447\u043e\u043a \u043f\u0435\u0440\u0435\u043a\u0440\u0430\u0448\u0438\u0432\u0430\u0435\u0442 \u043b\u043e\u0433\u043e\u0442\u0438\u043f");
  clicks(2500, 2650); doubleClick();
  assert.deepEqual([shell.ponyShown, unlocked], [false, []], "\u0434\u0432\u0430 \u0449\u0435\u043b\u0447\u043a\u0430 \u2014 \u043d\u0435 \u043f\u043e\u043d\u0438");
  t.mock.timers.tick(1500);
  assert.equal(shell.logoColor, "", "\u0446\u0432\u0435\u0442 \u0432\u043e\u0437\u0432\u0440\u0430\u0449\u0430\u0435\u0442\u0441\u044f \u0447\u0435\u0440\u0435\u0437 1,5 \u0441");
  // \u0422\u0440\u0438 \u0449\u0435\u043b\u0447\u043a\u0430, \u043d\u043e \u043c\u0435\u0436\u0434\u0443 \u043d\u0438\u043c\u0438 \u0431\u043e\u043b\u044c\u0448\u0435 650 \u043c\u0441, \u2014 \u043d\u0435\u0442.
  clicks(5000, 5700, 6400);
  clicks(9000, 9300, 10000);
  assert.deepEqual([shell.ponyShown, unlocked], [false, []], "\u043c\u0435\u0434\u043b\u0435\u043d\u043d\u044b\u0435 \u0442\u0440\u0438 \u0449\u0435\u043b\u0447\u043a\u0430 \u2014 \u043d\u0435 \u043f\u043e\u043d\u0438");
  // \u0422\u0440\u0438 \u0431\u044b\u0441\u0442\u0440\u044b\u0445 \u0449\u0435\u043b\u0447\u043a\u0430 (\u043c\u0435\u0436\u0434\u0443 \u0432\u0442\u043e\u0440\u044b\u043c \u0438 \u0442\u0440\u0435\u0442\u044c\u0438\u043c \u0431\u0440\u0430\u0443\u0437\u0435\u0440 \u0448\u043b\u0451\u0442 dblclick) \u2014 \u043f\u043e\u043d\u0438 \u0438 \u0434\u043e\u0441\u0442\u0438\u0436\u0435\u043d\u0438\u0435.
  clicks(20000, 20200); doubleClick(); clicks(20450);
  assert.equal(shell.ponyShown, true);
  assert.deepEqual(unlocked, ["back-to-past"]);
  clicks(20700);
  assert.deepEqual(unlocked, ["back-to-past"], "\u0447\u0435\u0442\u0432\u0451\u0440\u0442\u044b\u0439 \u0449\u0435\u043b\u0447\u043e\u043a \u043d\u0430\u0447\u0438\u043d\u0430\u0435\u0442 \u0441\u0447\u0451\u0442 \u0437\u0430\u043d\u043e\u0432\u043e");
  t.mock.timers.tick(1000);
  clicks(21500, 21700, 21900);
  t.mock.timers.tick(450);
  assert.equal(shell.ponyShown, false, "\u043f\u043e\u043a\u0430 \u043f\u043e\u043d\u0438 \u043d\u0430 \u044d\u043a\u0440\u0430\u043d\u0435, \u043d\u043e\u0432\u044b\u0435 \u0449\u0435\u043b\u0447\u043a\u0438 \u0435\u0433\u043e \u043d\u0435 \u043f\u0435\u0440\u0435\u0437\u0430\u043f\u0443\u0441\u043a\u0430\u044e\u0442");
  clicks(30000, 30150, 30300);
  assert.equal(shell.ponyShown, true, "\u0438 \u043a\u0430\u0436\u0434\u044b\u0435 \u0441\u043b\u0435\u0434\u0443\u044e\u0449\u0438\u0435 \u0442\u0440\u0438 \u0449\u0435\u043b\u0447\u043a\u0430 \u2014 \u0441\u043d\u043e\u0432\u0430");
  const vue = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/LauncherShell.vue"), "utf8");
  assert.match(vue, /@click="onLogoClick"\s+@dblclick="onLogoDoubleClick"/);
  assert.equal(vue.includes("logoRepaints"), false, "\u0441\u0442\u0430\u0440\u043e\u0433\u043e \u043f\u0443\u0442\u0438 \u00ab\u0442\u0440\u0435\u0442\u0438\u0439 \u0434\u0432\u043e\u0439\u043d\u043e\u0439 \u0449\u0435\u043b\u0447\u043e\u043a\u00bb \u043d\u0435\u0442");
  const css = fs.readFileSync(path.join(__dirname, "../src/renderer/components/Launcher/LauncherShell.vue"), "utf8");
  assert.match(css, /\.egg-pony \{[^}]*pointer-events: none;/);
  assert.match(css, /\.egg-cheat \{[^}]*pointer-events: none;/);
});

test("тема «Бумага»: четыре акцента, сохранение в настройках лаунчера, в CSS только цвета и только под [data-theme=paper]", () => {
  const dir = path.join(__dirname, "../src/renderer");
  const read = (file) => fs.readFileSync(path.join(dir, file), "utf8");
  // launcher-themes.js — ES-модуль: убираем export и возвращаем нужное.
  const themes = new Function(read("components/Launcher/launcher-themes.js").replace(/^export /gm, "") +
    "\nreturn { THEMES, PAPER_ACCENTS, themeById, themeVars };")();
  const paper = themes.themeById("paper");
  assert.equal(paper.id, "paper");
  assert.deepEqual(paper.accents.map((item) => item.id), ["clay", "rose", "amber", "sea"]);
  assert.equal(themes.themeVars("paper", "sea")["--accent"], "#80B2BA");
  assert.equal(themes.themeVars("paper", "нет такого")["--accent"], "#D5876D", "неизвестный акцент — глина");
  assert.equal(themes.themeVars("paper", "rose")["--btn-ready-glow"], "rgba(215, 138, 168, 0.32)", "свечение «Играть» — цветом акцента");
  // Формы как у темы HLSR: тема задаёт только цвета, без радиусов, размытия и теней.
  for (const name of Object.keys(themes.themeVars("paper"))) assert.ok(!/radius|blur|shadow|chrome|rail-fill/.test(name), name);
  assert.equal(themes.themeVars("lambda", "rose"), themes.themeById("lambda").vars, "у других тем акцент не подмешивается");
  for (const theme of themes.THEMES) assert.ok(!theme.accents || theme.id === "paper", theme.id);
  assert.match(read("components/Launcher/launcher-prefs.js"), /paperAccent: "clay",/);
  assert.match(read("components/Launcher/LauncherShell.vue"), /\.\.\.themeVars\(this\.prefs\.launcherTheme, this\.prefs\.paperAccent\)/);
  assert.match(read("components/Launcher/SettingsModal.vue"), /resetDraft\(\) \{[\s\S]*?paperAccent: this\.prefs\.paperAccent/, "сброс держит применённый акцент");
  for (const lang of ["ru", "en"]) {
    const locale = read(`utils/locales/${lang}.locale.js`);
    for (const key of [paper.title, paper.hint, "#L_THEME_PAPER_ACCENT", ...paper.accents.map((item) => item.title)]) {
      assert.ok(locale.includes(`"${key}"`), `${key} ${lang}`);
    }
  }
  // Все правила темы в launcher.css начинаются с корня в этой теме.
  const css = read("assets/css/launcher.css");
  const block = css.slice(css.indexOf("*/", css.indexOf("Тема «Бумага» (paper)")) + 2);
  const selectors = block.replace(/\/\*[\s\S]*?\*\//g, "").match(/[^{}]+(?=\{)/g).map((s) => s.trim());
  assert.ok(selectors.length >= 8);
  // Цветные метки — яркими, как в других темах: приглушённые терялись на коричневом (владелец 10.11).
  assert.doesNotMatch(selectors.join("\n"), /is-online|player__dot|dpl__marker|toasts__item|sponsors|profile__window|tier-|launcher-bg__orb/);
  for (const name of ["--l-level-ok", "--l-level-warning", "--l-level-error"]) assert.equal(themes.themeVars("paper")[name], undefined, name);
  for (const group of selectors) {
    for (const one of group.split(/,(?![^(]*\))/)) assert.match(one.trim(), /^\.launcher-root\[data-theme="paper"\]/, one);
  }
  // …и меняют только цвет: никаких размеров, отступов, шрифтов и display.
  const props = [...block.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([a-z-]+)\s*:[^;{}]*;/g)].map((m) => m[1]);
  for (const prop of props) assert.match(prop, /^(--[a-z-]+|color|background|background-color|box-shadow|filter)$/, prop);
});
test("раскладка 11.11: выдвинутая колонка сдвигает и сужает страницу во всех разделах, как в «Демо», а не ложится поверх", () => {
  const read = (name) => fs.readFileSync(path.join(__dirname, `../src/renderer/components/Launcher/${name}.vue`), "utf8");
  const shell = read("LauncherShell");
  assert.ok(!shell.includes("rail-float") && !shell.includes(".game-rail"), "у «Главной» и «Персонализации» нет режима «колонка поверх страницы»");

  // Страница любого раздела — сосед колонки в одной flex-строке .launcher-frame.
  const classes = (node) => ((node.props || []).find((p) => p.type === 6 && p.name === "class") || { value: { content: "" } }).value.content.split(/\s+/);
  const find = (node, match) => {
    if (node.type === 1 && match(node)) return node;
    for (const child of node.children || []) {
      const hit = find(child, match);
      if (hit) return hit;
    }
    return null;
  };
  const frame = find(parse(shell).descriptor.template.ast, (node) => classes(node).includes("launcher-frame"));
  const row = frame.children.filter((n) => n.type === 1).flatMap((n) => (["keep-alive", "transition"].includes(n.tag) ? n.children.filter((c) => c.type === 1) : [n]));
  const rail = row.findIndex((n) => n.tag === "GameSidebar");
  assert.ok(rail >= 0);
  for (const [tag, title] of [["HomePanel", "Главная"], ["GameConfigsPanel", "Персонализация"], ["DemoPanel", "Демо"]]) {
    assert.ok(row.findIndex((n) => n.tag === tag) > rail, `${title} — в строке за колонкой`);
    assert.match(read(tag), /^<template>[\s\S]*?<section\s+class="launcher-main /, `${title}: корень — .launcher-main`);
  }
  assert.ok(row.findIndex((n) => n.tag === "div" && classes(n).includes("launcher-main")) > rail, "страница игры и LiveSplit — в строке за колонкой");

  const style = shell.slice(shell.indexOf("<style"));
  assert.match(style, /\.launcher-frame \{\s*position: absolute;\s*inset: 0;\s*display: flex;\s*\}/);
  assert.match(style, /\.launcher-main \{\s*flex: 1;[^}]*min-width: 0;/, "страница сужается вместе с колонкой");
  const sidebar = read("GameSidebar");
  assert.match(sidebar, /\.game-rail \{\s*position: relative;\s*width: var\(--l-rail-w\);\s*min-width: var\(--l-rail-w\);/, "колонка в потоке");
  assert.match(sidebar, /transition: width var\(--l-dur-swipe\) var\(--l-ease-swipe\), min-width var\(--l-dur-swipe\)/, "страница едет тем же свайпом");
  assert.match(sidebar, /&\.is-open \{\s*width: var\(--l-rail-open-w\);\s*min-width: var\(--l-rail-open-w\);/);

  // Кнопка колонки работает в каждом разделе: в «Играх» — сохранённое положение, на «Главной» и в «Персонализации» — до ухода с вкладки.
  const railOpen = method("railOpen");
  const toggleRail = method("toggleRail");
  for (const section of ["home", "configs"]) {
    const vm = { section, railPeek: false, prefs: { railExpanded: false } };
    assert.equal(railOpen.call(vm), false);
    toggleRail.call(vm);
    assert.equal(railOpen.call(vm), true, section);
  }
  assert.equal(railOpen.call({ section: "games", railPeek: false, prefs: { railExpanded: true } }), true);

  // Узкая страница (окно 1152 и выдвинутая колонка): подвал «Персонализации» — значками,
  // кнопки предпросмотра и нижней строки внешнего вида переносятся, а не обрезаются.
  assert.match(read("GameConfigsPanel"), /@media \(max-width: 1400px\) \{ @include icon-footer; \}\s*@media \(max-width: 1560px\) \{ \.rail-open & \{ @include icon-footer; \} \}/);
  assert.match(read("GhostingSettingsPreview"), /^\s*:deep\(\.gv__bar\) \{ flex-wrap: wrap; row-gap: 6px; \}/m);
  assert.match(read("GamePersonalization"), /&__footer \{\s*flex: none;\s*display: flex;\s*flex-wrap: wrap;/);
});
