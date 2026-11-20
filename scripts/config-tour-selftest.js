"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "../src/renderer");
const read = (p) => fs.readFileSync(path.join(root, p), "utf8");
const tour = require("../src/renderer/components/Launcher/config-tour");

const ids = (list) => list.map((step) => step.id);

test("шаги обучения: GoldSrc — персонализация, конфиг, клавиатура, скрипты, запуск, демо", () => {
  assert.deepEqual(ids(tour.stepsFor("goldsrc", "hl1-won")), [
    "configs-tab", "configs-games", "gs-personal", "gs-configs", "configs-import", "configs-export",
    "gs-keyboard", "gs-scripts", "gs-launch", "gs-demos",
  ]);
});

test("шаги обучения: у Source нет скриптов и демо, у Ghosting есть призраки", () => {
  const source = ids(tour.stepsFor("source", "hl2-4260"));
  // У HL2 нет персонализации игры, демо и призраков.
  assert.deepEqual(source, [
    "configs-tab", "configs-games", "gs-configs", "configs-import", "configs-export", "gs-keyboard", "gs-launch",
  ]);
  assert.deepEqual(ids(tour.stepsFor("source", "ghosting-renewed")), [
    "configs-tab", "configs-games", "gs-personal", "gs-configs", "configs-import", "configs-export",
    "gs-keyboard", "gs-launch", "gs-demos", "gs-ghosts",
  ]);
  assert.ok(!ids(tour.stepsFor("source-old", "hl2-2707")).includes("gs-scripts"));
  for (const list of [source, ids(tour.stepsFor("goldsrc", "hl1-won"))]) assert.ok(!list.includes("personal"));
});

test("шаги обучения: раздел списка слева (pageSection) для каждого шага", () => {
  const want = {
    "configs-tab": null, "configs-games": null, "gs-personal": "personal", "gs-configs": "configs",
    "configs-import": "configs", "configs-export": "configs", "gs-keyboard": "keyboard",
    "gs-scripts": "scripts", "gs-launch": "launch", "gs-demos": "demos", "gs-ghosts": "ghosts",
  };
  for (const [engine, game] of [["goldsrc", "hl1-won"], ["source", "ghosting-renewed"], ["source", "hl2-4260"]])
    for (const step of tour.stepsFor(engine, game)) assert.equal(step.pageSection, want[step.id], step.id);
  // ConfigTour просит раздел у оболочки, оболочка — у панели, панель — у окна игры.
  const vue = read("components/Launcher/ConfigTour.vue");
  assert.match(vue, /if \(step\.pageSection\) this\.\$emit\("page-section", step\.pageSection\)/);
  assert.match(read("components/Launcher/LauncherShell.vue"), /@page-section="onConfigTourPageSection"[\s\S]*?\$refs\.configsPanel;[\s\S]*?panel\.selectPageSection\(id\)/);
  // Несохранённые правки: раздел не переключается и вопрос не открывается под щитом обучения.
  assert.match(read("components/Launcher/GameConfigsPanel.vue"),
    /selectPageSection\(id\) \{[^]*?if \(id !== gs\.activeSection && \(this\.unsaved \|\| this\.dirty\(\)\)\) return false;\s*return gs\.selectSection\(id\);/);
});

test("список слева: nav без tablist, aria-current, якорь gs-personal, подпункт не повторяет пункт", () => {
  const gsm = read("components/Launcher/GameSettingsModal.vue");
  assert.ok(gsm.includes(`:role="embedded ? null : 'tablist'"`) && gsm.includes(`:role="embedded ? null : 'tab'"`));
  assert.ok(gsm.includes(`:aria-current="embedded && activeSection === s.id ? 'true' : null"`));
  assert.ok(gsm.includes(`:data-tour="'gs-' + s.id"`));
  assert.ok(read("components/Launcher/GhostingSettingsExtras.vue").includes("title: '#L_GS_S_LOOK'"));
  assert.ok(!read("components/Launcher/GamePersonalization.vue").includes('data-tour="personal"'));
});

test("шаги обучения: раздел и якорь", () => {
  for (const step of tour.stepsFor("goldsrc", "hl1-won")) {
    assert.equal(step.anchor, step.id);
    // Первый шаг подсвечивает вкладку в шапке в текущем разделе (section: null).
    assert.equal(step.section, step.id === "configs-tab" ? null : "configs");
    assert.equal(step.title, `#L_TOUR_${step.id.toUpperCase().replace(/-/g, "_")}_TITLE`);
    assert.equal(step.text, `#L_TOUR_${step.id.toUpperCase().replace(/-/g, "_")}_TEXT`);
  }
  assert.deepEqual(tour.stepsFor("unknown", "x").map((s) => s.id).slice(0, 2), ["configs-tab", "configs-games"]);
});

test("тексты шагов и кнопок есть в ru и en", () => {
  const keys = new Set(["#L_TOUR_BACK", "#L_TOUR_NEXT", "#L_TOUR_SKIP", "#L_TOUR_DONE"]);
  for (const [engine, game] of [["goldsrc", "hl1-won"], ["source", "ghosting-renewed"], ["source", "hl2-4260"]])
    for (const step of tour.stepsFor(engine, game)) { keys.add(step.title); keys.add(step.text); }
  for (const lang of ["ru", "en"]) {
    const locale = read(`utils/locales/${lang}.locale.js`);
    for (const key of keys) assert.match(locale, new RegExp(`"${key}": "[^"]+"`), `${key} ${lang}`);
    assert.ok(!/—|–/.test(locale.split("\n").filter((l) => l.includes("#L_TOUR_")).join("\n")), "без тире в текстах обучения " + lang);
    assert.ok(!locale.includes("#L_TOUR_PERSONAL_") && locale.includes('"#L_GS_S_LOOK"'), "ключи " + lang);
    assert.ok(!/онфиги»|Configs tab/.test(locale), "вкладка называется «Персонализация» " + lang);
  }
});

test("launcher-prefs: configTourSeen по умолчанию false", () => {
  assert.match(read("components/Launcher/launcher-prefs.js"), /configTourSeen: false,/);
});

test("ConfigTour: подсветка по getBoundingClientRect, Esc, фокус, без анимаций", () => {
  const vue = read("components/Launcher/ConfigTour.vue");
  assert.ok(vue.includes("getBoundingClientRect"));
  assert.ok(vue.includes('[data-tour="'));
  assert.match(vue, /e\.key === "Escape"/);
  assert.ok(vue.includes("trapFocus"));
  assert.ok(vue.includes("prefers-reduced-motion") && vue.includes(".no-anim"));
  for (const key of ["#L_TOUR_BACK", "#L_TOUR_NEXT", "#L_TOUR_SKIP"]) assert.ok(vue.includes(key), key);
  assert.ok(vue.includes('addEventListener("resize"') && vue.includes('addEventListener("scroll"'));
  assert.match(vue, /role="dialog"/);
});

test("GameSettingsModal: якорь gs-personal на пункте списка слева", () => {
  assert.ok(read("components/Launcher/GameSettingsModal.vue").includes(`:data-tour="'gs-' + s.id"`));
});

test("оболочка: обучение показывается при !configTourSeen, после «С нуля» и по «?»", () => {
  const shell = read("components/Launcher/LauncherShell.vue");
  assert.match(shell, /<ConfigTour[\s\S]*?v-if="configTour"[\s\S]*?@section="section = \$event"[\s\S]*?@done="onConfigTourDone"/);
  assert.ok(shell.includes("!this.prefs.configTourSeen"));
  assert.ok(!/onConfigsHelp\(\) \{\}/.test(shell));
  assert.match(shell, /onConfigsHelp\(gameId\) \{[^]*?this\.startConfigTour\(/);
  assert.match(shell, /onConfigTourDone\(\) \{[^]*?configTourSeen = true/);
  // Без установленной игры и при открытых окнах обучение откладывается.
  assert.match(shell, /configTourDue\(\) \{[^]*?configGames\.length[^]*?overlayOpen/);
  // «С нуля»: флаг сбрасывается, обучение стартует.
  assert.match(shell, /configTourRequested\(on\) \{\s*if \(!on\) return;\s*this\.configTourRequested = false;\s*this\.startConfigTour\(this\.configsGameId\);/);
  assert.ok(shell.includes('@help="onConfigsHelp"'));
});

test("панель «Конфиги» передаёт игру в «?»", () => {
  assert.ok(read("components/Launcher/GameConfigsPanel.vue").includes("$emit('help', gameId)"));
});

test("обучение: возврат раздела, короткое ожидание, щит без карточки, флаг приветствия на пути сбоя", () => {
  const shell = read("components/Launcher/LauncherShell.vue");
  const vue = read("components/Launcher/ConfigTour.vue");
  // Автозапуск запоминает раздел и не уводит игрока во «Конфиги» заранее; «?» и «С нуля» остаются там.
  assert.match(shell, /const back = this\.section;[^]*?this\.startConfigTour\(game\.id, back\);/);
  assert.ok(shell.includes('startConfigTour(gameId, back = "configs")'));
  assert.match(shell, /onConfigTourDone\(\) \{\s*const back = this\.configTour \? this\.configTour\.back : "configs";\s*const pageSection = this\.configTour \? this\.configTour\.pageSection : "";\s*this\.configTour = null;\s*this\.section = back;\s*if \(pageSection\) this\.onConfigTourPageSection\(pageSection\);/);
  // Ожидание якоря короткое, щит без карточки не ловит нажатия.
  assert.match(vue, /const WAIT_MS = 400;/);
  assert.match(vue, /&\.idle \{\s*pointer-events: none;/);
  assert.ok(vue.includes("!!step.section && step.section !== this.section"));
  // welcomeChecked ставится и после сбоя get-library-path.
  assert.match(shell, /async checkLibrarySetup\(\) \{\s*try \{[^]*?\} catch \(error\) \{[^]*?\} finally \{[^]*?this\.welcomeChecked = true;/);
});
