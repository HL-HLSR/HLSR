"use strict";

// node --test scripts/hl1-config-check-selftest.js

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const path = require("path");
const { checkConfig } = require("../src/shared/hl1-config-check");
const { build } = require("./build-hl1-whitelist");

const WHITELIST = path.join(__dirname, "..", "src", "shared", "hl1-whitelist.json");
const FIXTURES = path.join(__dirname, "fixtures", "hl1-whitelist");
const CONFIGS = path.join(__dirname, "fixtures", "single-config", "hl1");

const check = (text, extra = {}, category) => checkConfig({ files: Object.assign({ "a.cfg": text }, extra), root: "a.cfg", category });

test("таблица: валидный JSON с нужными разделами", () => {
  const data = JSON.parse(fs.readFileSync(WHITELIST, "utf8"));
  assert.deepEqual(Object.keys(data), ["commands", "cvars", "patterns", "skill", "source"]);
  assert.ok(Object.keys(data.cvars).length > 100 && Object.keys(data.commands).length > 100);
  assert.ok(data.patterns.length >= 4 && Object.keys(data.skill).length > 100);
  const rules = new Set(["allowed", "banned", "scripted", "scripted-only", "no-script-seq", "category", "thirdperson", "steam", "run-start", "values"]);
  for (const entry of [...Object.values(data.cvars), ...Object.values(data.commands), ...data.patterns]) {
    assert.ok(rules.has(entry.rule), entry.rule);
  }
  assert.equal(data.cvars.sv_cheats.rule, "banned");
  assert.equal(data.cvars.brightness.rule, "allowed", "яркость разрешена владельцем таблицы с 07.11");
  // Надписи таблицы разведены по двум правилам (R25).
  assert.equal(data.commands.wait.rule, "scripted-only");
  assert.equal(data.cvars.cl_pitchup.rule, "scripted-only");
  assert.equal(data.commands["+forward"].rule, "no-script-seq");
  assert.equal(Object.values(data.commands).filter((e) => e.rule === "scripted").length, 0);
  assert.deepEqual(data.cvars.bxt_hud_viewangles.values, ["1", "2"]);
  assert.equal(data.skill.sk_9mmar_bullet1, "3");
  assert.equal(fs.readFileSync(WHITELIST, "utf8").includes("\r"), false);
});

test("генератор детерминирован и даёт тот же файл по сохранённым CSV", () => {
  const read = (name) => fs.readFileSync(path.join(FIXTURES, name), "utf8");
  const colors = JSON.parse(read("main-colors.json"));
  const committed = fs.readFileSync(WHITELIST, "utf8");
  const fetchedAt = JSON.parse(committed).source.fetchedAt;
  const warnings = [];
  const first = build(read("main.csv"), read("skill.csv"), colors, fetchedAt, (text) => warnings.push(text));
  assert.equal(build(read("main.csv"), read("skill.csv"), colors, fetchedAt, () => {}), first);
  assert.equal(first, committed);
  // Известные расхождения цвета и текста: зелёные ячейки «только для бинда старта рана».
  assert.deepEqual(warnings.map((w) => /«([^»]+)»/.exec(w)[1]).sort(), ["bxt_timer_reset", "bxt_timer_start", "map c1a0"]);
});

test("запрещённая переменная со значением по умолчанию — не нарушение", () => {
  assert.equal(check("sv_cheats 0\ncl_lc 1\nhost_framerate 0\nhost_framerate 0.0\n").findings.length, 0);
  const bad = check("sv_cheats 1\nbind j sv_cheats 0\n");
  assert.deepEqual(bad.findings.map((f) => [f.command, f.level]), [["sv_cheats 1", "error"]]);
});

test("зацикленные alias и exec не виснут", () => {
  assert.equal(check('alias loop "loop"\nalias a b\nalias b a\nloop\n').findings.length, 0);
  const cycle = check("exec b.cfg\nexec a.cfg\n", { "b.cfg": "exec c.cfg\nsv_cheats 1\n", "c.cfg": "exec b.cfg\nexec a.cfg\n" });
  assert.deepEqual(cycle.findings.map((f) => f.file), ["b.cfg"]);
});

test("exec с кавычками и другим регистром", () => {
  const result = check('exec "B.cfg"\nexec "sub\\C"\n', { "b.cfg": "sv_cheats 1\n", "sub/c.cfg": "bxt_autojump 1\n" });
  assert.deepEqual(result.findings.map((f) => [f.file, f.rule]), [["b.cfg", "banned"], ["sub/c.cfg", "scripted-only"]]);
});

test("brightness разрешён: ни error, ни другой находки", () => {
  assert.equal(check("brightness 2\nBrightness 1\n").findings.length, 0);
});

test("запрещённая переменная — error", () => {
  const result = check("sv_cheats 2\n");
  assert.equal(result.findings.length, 1);
  assert.equal(result.findings[0].level, "error");
  assert.equal(result.findings[0].rule, "banned");
  assert.equal(result.findings[0].line, 1);
  assert.equal(result.findings[0].context, "top level");
});

test("категории, thirdperson и steam — warning в обеих категориях", () => {
  const text = "bxt_force_duck 1\ncam_idealyaw 0\nfps_override 1\nthirdperson\n";
  for (const category of ["scriptless", "scripted"]) {
    assert.deepEqual(check(text, {}, category).findings.map((f) => [f.rule, f.level]), [
      ["category", "warning"], ["thirdperson", "warning"], ["steam", "warning"], ["thirdperson", "warning"],
    ], category);
  }
});

test("scripted-only: в Scripted тишина, в Scriptless — error с причиной", () => {
  const text = "w 10\nwait\nbxt_append x\ncl_pitchup 89\nbxt_autojump 1\nbxt_autopause 1\n+bxt_tas_autojump\n+bxt_tas_ducktap\n+bxt_tas_jumpbug\nbxt_tas_ducktap_priority 1\n";
  assert.equal(check(text, {}, "scripted").findings.length, 0);
  const result = check(text);
  assert.equal(result.findings.length, 10);
  assert.ok(result.findings.every((f) => f.level === "error" && f.rule === "scripted-only" && f.note === "В Scriptless нельзя (только Scripted)"));
  assert.equal(result.stats.errors, 10);
  // Категория по умолчанию — Scriptless; чужое значение тоже.
  assert.equal(result.stats.category, "scriptless");
  assert.equal(check(text, {}, "bogus").stats.category, "scriptless");
  assert.equal(check(text, {}, "scripted").stats.category, "scripted");
  // В теле alias и bind правило то же.
  assert.deepEqual(check('alias a "w 10;wait"\nbind x "wait"\n', {}, "scripted").findings, []);
  assert.equal(check('alias a "w 10;wait"\nbind x "wait"\n').findings.length, 3);
});

test("no-script-seq: error в обеих категориях, только в теле из двух и более команд", () => {
  for (const category of ["scriptless", "scripted"]) {
    // Одиночное тело и верхний уровень — тишина.
    assert.deepEqual(check("alias stmove +forward\nbind w +forward\nbind s \"+back\"\n+forward\n+moveleft; +moveright\n", {}, category).findings, [], category);
    const multi = check('alias run "+forward; +attack"\nbind d "+moveright;+attack2"\nbind a "bind b +left; +right"\n', {}, category);
    assert.deepEqual(multi.findings.map((f) => [f.command, f.level, f.rule]), [
      ["+forward", "error", "no-script-seq"], ["+moveright", "error", "no-script-seq"], ["+right", "error", "no-script-seq"],
    ], category);
    assert.equal(multi.stats.errors, 3);
  }
});

test("значение вне values — warning, из values — тишина", () => {
  assert.equal(check("bxt_hud_viewangles 1\nbxt_hud_viewangles 2\nimpulse 100\n").findings.length, 0);
  // Значение по умолчанию таблицы (0 — выключено) не нарушение, как у banned.
  assert.equal(check("bxt_hud_viewangles 0\n").findings.length, 0);
  const bad = check("bxt_hud_viewangles 3\nimpulse 101\n");
  assert.deepEqual(bad.findings.map((f) => f.rule), ["values", "values"]);
});

test("sk_ должен равняться значению по умолчанию", () => {
  assert.equal(check("sk_9mmar_bullet1 3\nsk_9mmAR_bullet1 3.0\n").findings.length, 0);
  const bad = check("sk_9mmar_bullet1 99\nsk_nonexistent 1\n");
  assert.deepEqual(bad.findings.map((f) => [f.rule, f.level]), [["skill", "warning"], ["unknown", "error"]]);
});

test("неизвестная команда — error, регистр не важен, комментарии не считаются", () => {
  const result = check("// totallyfake 1\nTotallyFake 1 // тоже\nSENSITIVITY 3\nSv_Cheats 1\n");
  assert.deepEqual(result.findings.map((f) => [f.command, f.rule]), [["TotallyFake 1", "unknown"], ["Sv_Cheats 1", "banned"]]);
});

test("свои alias и их +name/-name не неизвестные; тело alias проверяется", () => {
  const result = check('alias +go "+forward;alias zzz wait"\nalias -go "-forward"\nbind f +go\n+go\n-go\ngo\nzzz\n', {}, "scripted");
  // +forward в связке из двух команд — error и в Scripted; wait в Scripted разрешён, но «alias zzz» — вложенный.
  assert.deepEqual(result.findings.map((f) => [f.command, f.context]), [
    ["+forward", "alias +go"],
  ]);
  const scriptless = check('alias +go "+forward;alias zzz wait"\nalias -go "-forward"\nbind f +go\n+go\n-go\ngo\nzzz\n');
  assert.deepEqual(scriptless.findings.map((f) => [f.command, f.context]), [
    ["+forward", "alias +go"],
    ["wait", "alias +go > alias zzz"],
  ]);
});

test("bind внутри bind и alias, exec внутри них", () => {
  const files = { "b.cfg": "sv_cheats 1\nexec c.cfg\n", "c.cfg": "bxt_autopause 1\n" };
  const result = check('alias y "bind j bind k sv_cheats 2; exec b.cfg"\nbind m "bind n exec c.cfg"\n', files);
  const sv_cheats = result.findings.filter((f) => f.command.startsWith("sv_cheats"));
  assert.deepEqual(sv_cheats.map((f) => [f.file, f.context]), [
    ["a.cfg", "alias y > bind j > bind k"],
    ["b.cfg", "exec из alias y"],
  ]);
  assert.equal(result.findings.some((f) => f.file === "c.cfg" && f.rule === "scripted-only"), true);
  // Тот же путь структурой: язык текста выбирает окно, а не модуль проверки.
  assert.deepEqual(sv_cheats.map((f) => f.trail), [
    [{ kind: "alias", name: "y" }, { kind: "bind", name: "j" }, { kind: "bind", name: "k" }],
    [{ kind: "exec", via: [{ kind: "alias", name: "y" }] }],
  ]);
  assert.deepEqual(check("sv_cheats 1\n").findings[0].trail, []);
});

test("exec: цепочка, пропавший файл — info, повтор не зацикливает", () => {
  const files = { "b.cfg": "exec a.cfg\nbxt_autojump 1\n" };
  const result = check("exec b.cfg\nexec nope\n", files);
  assert.deepEqual(result.findings.map((f) => [f.file, f.level, f.rule]), [
    ["b.cfg", "error", "scripted-only"],
    ["a.cfg", "info", "missing"],
  ]);
  assert.equal(result.findings[1].note, "файл не найден");
});

test("старт рана: map c1a0 в теле alias/bind молчит, таймер без него и на верхнем уровне — info", () => {
  const quiet = check('alias start "bxt_timer_reset; map c1a0; bxt_timer_start"\nbind p "map c1a0;bxt_timer_start"\n');
  assert.equal(quiet.findings.length, 0);
  const loud = check('alias t "bxt_timer_start"\nbxt_timer_reset\nmap c1a0\nmap c1a1\n');
  assert.deepEqual(loud.findings.map((f) => [f.command, f.level, f.rule]), [
    ["bxt_timer_start", "info", "run-start"],
    ["bxt_timer_reset", "info", "run-start"],
    ["map c1a0", "info", "run-start"],
    ["map c1a1", "error", "banned"],
  ]);
});

test("запрещённый шаблон bxt_tas_, но не bxt_tas_ducktap_priority", () => {
  const result = check("bxt_tas_norefresh_until_stop_frame 1\nbxt_tas_ducktap_priority 1\n_bxt_tas_x 1\nweapon_gauss\n-foo\n");
  assert.deepEqual(result.findings.map((f) => f.rule), ["banned", "scripted-only", "banned"]);
});

// --- конфиг владельца -------------------------------------------------------

const owner = (category) => {
  const files = {};
  for (const name of fs.readdirSync(CONFIGS)) files[name.toLowerCase()] = fs.readFileSync(path.join(CONFIGS, name), "latin1");
  return checkConfig({ files, root: "1.cfg", category });
};
const at = (result, command, context) => result.findings.filter((f) => f.command === command && (!context || f.context === context));

test("конфиг владельца: brightness 2 — без находок, bxt_autojump 1 — error в Scriptless, тишина в Scripted", () => {
  const result = owner();
  assert.equal(at(result, "brightness 2").length, 0);
  const jump = at(result, "bxt_autojump 1");
  assert.ok(jump.some((f) => f.context === "top level" && f.level === "error" && f.rule === "scripted-only"));
  assert.equal(at(owner("scripted"), "bxt_autojump 1").length, 0);
});

test("конфиг владельца: cl_pitchup/cl_pitchdown и w в alias — error в Scriptless, тишина в Scripted", () => {
  const result = owner();
  assert.equal(at(result, "cl_pitchup 89", "alias fix")[0].level, "error");
  assert.equal(at(result, "cl_pitchdown 89", "alias fix")[0].rule, "scripted-only");
  assert.equal(at(result, "cl_pitchup 180", "alias _taubofunc")[0].level, "error");
  assert.equal(at(result, "w 10", "alias obboshoot")[0].rule, "scripted-only");
  const quiet = owner("scripted");
  for (const name of ["w", "wait", "cl_pitchup", "cl_pitchdown", "bxt_autojump", "bxt_append"]) {
    assert.equal(quiet.findings.filter((f) => f.command.split(" ")[0] === name).length, 0, name);
  }
});

test("конфиг владельца: +bxt_tas_jumpbug на клавише — error в Scriptless, тишина в Scripted", () => {
  const found = at(owner(), "+bxt_tas_jumpbug", "bind mouse5");
  assert.equal(found.length, 1);
  assert.equal(found[0].level, "error");
  assert.equal(found[0].rule, "scripted-only");
  assert.equal(at(owner("scripted"), "+bxt_tas_jumpbug").length, 0);
});

test("конфиг владельца: map c1a0 в start_run молчит, свои alias не неизвестные", () => {
  const result = owner("scripted");
  assert.equal(result.findings.filter((f) => f.command.startsWith("map c1a0")).length, 0);
  assert.equal(result.findings.filter((f) => f.context === "alias start_run" && f.rule === "run-start").length, 0);
  assert.equal(result.findings.filter((f) => f.context === "alias start_run" && f.level === "error").length, 0);
  const unknown = result.findings.filter((f) => f.rule === "unknown").map((f) => f.command.split(" ")[0]);
  for (const own of ["start_run", "fix", "+tau", "-tau", "obboshoot", "show_crosshair", "stskip", "stmove", "gonch", "p2ptau", "labosens"]) {
    assert.equal(unknown.includes(own), false, own);
  }
  assert.ok(result.stats.files >= 3);
});

// ---- Показ проверки во вкладке «Конфиги» (W2) ----

const os = require("os");
const cfgImport = require("../src/shared/cfg-import");
const cfgSearch = require("../src/main/game-delivery/cfg-search");
const gameBindsInstall = require("../src/main/game-delivery/game-binds-install");
const { groupFindings, ruleDetail, RULE_KEYS } = require("../src/shared/hl1-config-groups");

const GAME = "hl1-won";
const GAME_DIR = "Half-Life/valve_WON";
// Язык задаём явно: иначе cfg-search читает язык Steam этой машины.
const EN = { language: "english" };
const read = (name) => fs.readFileSync(path.join(CONFIGS, name), "latin1");

function install(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-rules-check-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const dir = cfgSearch.modDirOf(root, GAME_DIR, "goldsrc");
  fs.mkdirSync(dir, { recursive: true });
  return { root, dir };
}

// Импорт 1.cfg владельца так, как его делает окно: цепочка exec из фикстур, потом запись.
function importOwner(root) {
  const rootFile = { name: "1.cfg", text: read("1.cfg") };
  const files = {};
  for (let wanted = cfgImport.nextWanted({ engine: "goldsrc", root: rootFile, files }); wanted;
    wanted = cfgImport.nextWanted({ engine: "goldsrc", root: rootFile, files })) {
    files[wanted.key] = { name: wanted.name, target: wanted.name, text: read(wanted.name) };
  }
  const built = cfgImport.flattenImport({ engine: "goldsrc", root: rootFile, files });
  assert.equal(gameBindsInstall.importPlayerConfig(root, GAME, GAME_DIR, built, EN).status, "ok");
}

test("player-config-check: конфиг владельца после импорта — brightness не находка, группы сходятся со счётчиками", (t) => {
  const { root, dir } = install(t);
  fs.writeFileSync(path.join(dir, "userconfig.cfg"), "exec WON.cfg\r\n");
  importOwner(root);
  const scripted = gameBindsInstall.checkPlayerConfig(root, GAME, GAME_DIR, { ...EN, category: "scripted" });
  assert.deepEqual([scripted.stats.category, scripted.stats.errors, scripted.stats.warnings, scripted.stats.infos], ["scripted", 0, 0, 2], "Scripted: только две справки про старт рана");
  const result = gameBindsInstall.checkPlayerConfig(root, GAME, GAME_DIR, EN);
  assert.equal(result.stats.category, "scriptless", "без категории — Scriptless");
  assert.equal(result.status, "ok");
  assert.equal(result.file.split("/").pop(), "userconfig.cfg");
  assert.ok(result.stats.files >= 3, "файл игрока и вызываемые из bind/alias");
  const errors = result.findings.filter((item) => item.level === "error");
  assert.equal(result.findings.filter((item) => /^brightness/i.test(item.command)).length, 0);
  assert.deepEqual([result.stats.errors, result.stats.warnings, result.stats.infos], [293, 0, 2], "Scriptless: всё из «только Scripted» — ошибки");
  assert.equal(errors.every((item) => item.rule === "scripted-only"), true);
  assert.equal(result.findings.filter((item) => item.rule === "no-script-seq").length + scripted.findings.filter((item) => item.rule === "no-script-seq").length, 0,
    "в конфиге владельца +forward и родственные не стоят в связке из нескольких команд");
  const groups = groupFindings(result.findings);
  const count = (level) => groups.filter((group) => group.level === level).reduce((sum, group) => sum + group.places.length, 0);
  assert.equal(count("error"), result.stats.errors);
  assert.equal(count("warning"), result.stats.warnings);
  assert.equal(count("info"), result.stats.infos);
  assert.equal(errors.length, 293);
  assert.equal(groups[0].level, "error", "ошибки первыми");
  assert.ok(groups.length < result.findings.length / 4, "сотни находок схлопнуты в десятки групп");
  // Место открывается в редакторе: путь от папки игры с «/», такой файл есть на диске.
  for (const finding of result.findings) {
    assert.ok(finding.path && !finding.path.includes("\\") && fs.existsSync(path.join(root, ...finding.path.split("/"))), finding.path);
  }
  assert.ok(groups.find((group) => group.rule === "scripted-only").places.length > 1);
});

test("player-config-check: нет файла игрока — nothing, другая игра — unsupported", (t) => {
  const { root } = install(t);
  assert.equal(gameBindsInstall.checkPlayerConfig(root, GAME, GAME_DIR, EN).status, "nothing");
  assert.equal(gameBindsInstall.checkPlayerConfig(root, "hl2-2707", "hl2", EN).status, "unsupported");
});

test("player-config-check: канал в preload, обработчик в main, только чтение", () => {
  const src = (file) => fs.readFileSync(path.join(__dirname, "..", file), "utf8");
  assert.match(src("src/main/preload.js"), /"player-config-check",/);
  const main = src("src/main/game-delivery/index.js");
  assert.match(main, /ipcMain\.handle\("player-config-check", \(event, id, category\) => this\.playerConfigCheck\(id, category\)\)/);
  assert.match(main, /playerConfigCheck\(id, category = "scriptless"\) \{[\s\S]*?gameId !== "hl1-won"[\s\S]*?unsupported/);
  // Категория проверяется на границе IPC: чужое значение — BAD_REQUEST, а не молчаливый Scriptless.
  assert.match(main, /category !== "scriptless" && category !== "scripted"\) return \{ status: "error", code: "BAD_REQUEST" \}/);
  assert.match(main, /checkPlayerConfig\(installPath, gameId, config\.gameDirectory, \{ category \}\)/);
});

test("playerConfigCheck: категория проверяется, Scripted доходит до проверки", () => {
  const main = fs.readFileSync(path.join(__dirname, "..", "src", "main", "game-delivery", "index.js"), "utf8");
  const start = main.indexOf('playerConfigCheck(id, category = "scriptless") {');
  const body = main.slice(start, main.indexOf("\n  }\n", start) + 4);
  const calls = [];
  const self = { resolveGameId: () => "hl1-won", installedPathOf: () => "/g", gameBindsInstall: null };
  const run = new Function("manifestRegistry", "gameBindsInstall", "log", `return ({ ${body} });`)(
    { launchConfigOf: () => ({ gameDirectory: "d" }) },
    { checkPlayerConfig: (...args) => { calls.push(args[3]); return { status: "ok" }; } },
    { warn() {} },
  ).playerConfigCheck;
  assert.equal(run.call(self, "70", "scripted").status, "ok");
  assert.equal(run.call(self, "70").status, "ok");
  assert.deepEqual(calls, [{ category: "scripted" }, { category: "scriptless" }]);
  for (const bad of ["Scripted", "", null, 1, {}]) assert.deepEqual(run.call(self, "70", bad), { status: "error", code: "BAD_REQUEST" }, String(bad));
  assert.equal(calls.length, 2);
});

test("окно: переключатель Scriptless / Scripted, выбор запоминается и перепроверяет", () => {
  const root = path.join(__dirname, "..", "src", "renderer");
  const vue = (name) => fs.readFileSync(path.join(root, "components", "Launcher", name), "utf8");
  const check = vue("GameCfgCheck.vue");
  const modal = vue("GameSettingsModal.vue");
  const panel = vue("GameConfigsPanel.vue");
  const prefs = vue("launcher-prefs.js");
  // Переключатель: родные radio в radiogroup (клавиатура), подпись для чтения с экрана, цвет — токен темы.
  assert.match(check, /role="radiogroup"/);
  assert.match(check, /type="radio"/);
  assert.match(check, /@change="\$emit\('category', item\.id\)"/);
  assert.match(check, /\.hc__cat-item[\s\S]*?&:focus-within \{ outline: 2px solid var\(--accent-focus\)/);
  assert.match(check, /var\(--accent-soft\)/);
  assert.match(modal, /:category="ruleCategory"[\s\S]*?@category="setRuleCategory"/);
  assert.match(modal, /ruleCategory: prefsStore\.ruleCategoryOf\(\)/);
  assert.match(modal, /ipcRenderer\.invoke\("player-config-check", this\.gameId, this\.ruleCategory\)/);
  assert.match(modal, /setRuleCategory\(category\) \{[\s\S]*?prefsStore\.setRuleCategory\("hl1-won", this\.ruleCategory\)[\s\S]*?this\.loadRuleCheck\(\);/);
  // Итог импорта считает по той же сохранённой категории.
  assert.match(panel, /invoke\("player-config-check", id, prefsStore\.ruleCategoryOf\(\)\)/);
  // Хранение: по игре, по умолчанию Scriptless, чужие значения сводятся к нему, save() не затирает выбор.
  assert.match(prefs, /ruleCategory: \{\}/);
  assert.match(prefs, /ruleCategoryOf\(gameId = "hl1-won"\)/);
  assert.match(prefs, /write\(Object\.assign\(\{\}, prefs, \{ ruleCategory: kept\.ruleCategory, keyboardFull: kept\.keyboardFull \}\)\)/);
  for (const lang of ["ru", "en"]) {
    const locale = fs.readFileSync(path.join(root, "utils", "locales", `${lang}.locale.js`), "utf8");
    for (const key of ["#L_HC_CATEGORY", "#L_HC_RULE_SCRIPTED_ONLY", "#L_HC_RULE_NO_SEQ"]) assert.match(locale, new RegExp(`"${key}":`), `${lang} ${key}`);
  }
});

test("launcher-prefs: категория по игре, по умолчанию Scriptless, save() её не затирает", () => {
  const src = fs.readFileSync(path.join(__dirname, "..", "src", "renderer", "components", "Launcher", "launcher-prefs.js"), "utf8")
    .replace(/export const /g, "const ").replace(/export default (\{[^}]*\});/, "return $1;")
    .replace(/const \{ normalizeSpeedLimit \} = require\([^)]*\);/, "const normalizeSpeedLimit = (v) => v || 0;");
  const store = {};
  const localStorage = { getItem: (k) => (k in store ? store[k] : null), setItem: (k, v) => { store[k] = v; } };
  const prefs = new Function("localStorage", "window", src)(localStorage, { hlsrElectron: {} });
  assert.equal(prefs.ruleCategoryOf(), "scriptless");
  prefs.setRuleCategory("hl1-won", "scripted");
  assert.equal(prefs.ruleCategoryOf(), "scripted");
  assert.equal(prefs.ruleCategoryOf("other"), "scriptless");
  // Устаревший снимок настроек оболочки выбор не стирает.
  prefs.save(Object.assign(prefs.defaults(), { animations: false }));
  assert.equal(prefs.ruleCategoryOf(), "scripted");
  assert.equal(prefs.load().animations, false);
  prefs.setRuleCategory("hl1-won", "мусор");
  assert.equal(prefs.ruleCategoryOf(), "scriptless");
  store["hlsr-launcher-prefs"] = JSON.stringify({ ruleCategory: ["scripted"] });
  assert.equal(prefs.ruleCategoryOf(), "scriptless");
});

test("группы: по команде и правилу, ошибки первыми, детали значений из пояснения", () => {
  const f = (command, rule, level, line, note = "") => ({ file: "a.cfg", path: "x/a.cfg", line, command, rule, level, note, context: "top level" });
  const groups = groupFindings([
    f("w 10", "scripted", "warning", 1), f("W 20", "scripted", "warning", 2), f("sv_cheats 2", "banned", "error", 3),
    f("bxt_hud_viewangles 3", "values", "warning", 4, "допустимые значения: 1, 2"), f("sk_plr_dmg 1", "skill", "warning", 5, "значение должно быть по умолчанию: 3"),
  ]);
  assert.deepEqual(groups.map((group) => group.key), ["sv_cheats banned", "w scripted", "bxt_hud_viewangles values", "sk_plr_dmg skill"]);
  assert.equal(groups[1].places.length, 2);
  assert.equal(ruleDetail(groups[2]), "1, 2");
  assert.equal(ruleDetail(groups[3]), "3");
  assert.equal(ruleDetail(groups[0]), "");
  // Каждое нормализованное правило модуля проверки переведено.
  const rules = ["banned", "unknown", "scripted", "scripted-only", "no-script-seq", "category", "thirdperson", "steam", "values", "skill", "run-start", "missing"];
  for (const rule of rules) {
    assert.ok(RULE_KEYS[rule], rule);
    for (const lang of ["ru", "en"]) {
      assert.match(fs.readFileSync(path.join(__dirname, "..", "src", "renderer", "utils", "locales", `${lang}.locale.js`), "utf8"),
        new RegExp(`"${RULE_KEYS[rule]}":`), `${lang} ${rule}`);
    }
  }
});

test("окно: раздел «Проверка», строка итога импорта, уход из «Файлов cfg» с вопросом", () => {
  const vue = (name) => fs.readFileSync(path.join(__dirname, "..", "src", "renderer", "components", "Launcher", name), "utf8");
  const modal = vue("GameSettingsModal.vue");
  const editor = vue("GameConfigEditor.vue");
  const files = vue("GameCfgFiles.vue");
  const panel = vue("GameConfigsPanel.vue");
  const check = vue("GameCfgCheck.vue");
  // Блок: счётчики словом и значком, список групп раскрывается, место открывает файл.
  assert.match(check, /#L_HC_COUNT_ERRORS/);
  assert.match(check, /fa-times-circle[\s\S]*fa-exclamation-triangle/);
  assert.match(check, /aria-expanded/);
  assert.match(check, /\$emit\('open-place', place\)/);
  assert.match(check, /prefers-reduced-motion/);
  // Проверка идёт при открытии, после сохранения, импорта, возврата и по фокусу — через loadConfig и явные вызовы.
  assert.match(modal, /this\.configState = state;\s*this\.loadRuleCheck\(\);/);
  assert.match(modal, /onCfgFileSaved\(\) \{\s*this\.cfgFilesDirty = false;\s*this\.loadRuleCheck\(\);/);
  assert.match(modal, /this\.protonError = null;\s*this\.loadRuleCheck\(\);/);
  assert.match(modal, /ipcRenderer\.invoke\("player-config-check", this\.gameId, this\.ruleCategory\)/);
  assert.match(modal, /@open-place="showCheckPlace"/);
  assert.match(files, /async openAt\(file, line\)/);
  // Несохранённый текст не теряется молча при смене раздела (N2).
  assert.match(editor, /leaving && this\.canLeave && !\(await this\.canLeave\(\)\)/);
  assert.match(modal, /:can-leave="leaveCfgFiles"/);
  assert.match(modal, /if \(this\.askUnsaved\) return this\.askUnsaved\(\);/);
  assert.match(panel, /:ask-unsaved="settleUnsaved"/);
  // Строка итога импорта: только HL1, с кнопкой к блоку.
  assert.doesNotMatch(panel, /"hl1-won"/);
  assert.match(panel, /#L_HC_IMPORT_LINE/);
  assert.match(panel, /@click="showRules"/);
});

test("импорт сбрасывает editedPrevious игры", () => {
  const main = fs.readFileSync(path.join(__dirname, "..", "src", "main", "game-delivery", "index.js"), "utf8");
  assert.match(main, /dropImportedOverrides\(gameId\) \{[^}]*delete this\.editedPrevious\[gameId\]/);
});

test("окно: контекст без русского из модуля, цвета уровней — токены темы, сбой не прячет раздел", () => {
  const root = path.join(__dirname, "..", "src", "renderer");
  const vue = (name) => fs.readFileSync(path.join(root, "components", "Launcher", name), "utf8");
  const check = vue("GameCfgCheck.vue");
  const editor = vue("GameConfigEditor.vue");
  const modal = vue("GameSettingsModal.vue");
  assert.match(check, /place\.trail/);
  assert.doesNotMatch(check, /place\.context/);
  for (const file of [check, editor]) assert.doesNotMatch(file, /#(ff6b78|f2cf4a|5fd068)/i);
  const css = fs.readFileSync(path.join(root, "assets", "css", "launcher.css"), "utf8");
  for (const token of ["--l-level-error", "--l-level-warning", "--l-level-ok"]) {
    assert.ok(css.includes(`${token}:`), token);
    assert.ok(check.includes(`var(${token})`) && editor.includes(`var(${token})`), token);
  }
  assert.match(fs.readFileSync(path.join(root, "components", "Launcher", "launcher-themes.js"), "utf8"), /"--l-level-error"/);
  // Сбой перепроверки: прежний итог остаётся, без итога раздел показывает ошибку.
  assert.match(modal, /if \(!failed\) this\.ruleCheck = /);
  assert.match(modal, /this\.ruleCheckFailed \? \{ errors: 0, warnings: 0, failed: true \} : null/);
  assert.match(check, /#L_HC_FAILED/);
  assert.match(editor, /counts\.failed/);
  for (const lang of ["ru", "en"]) {
    const locale = fs.readFileSync(path.join(root, "utils", "locales", `${lang}.locale.js`), "utf8");
    for (const key of ["#L_HC_CTX_ALIAS", "#L_HC_CTX_BIND", "#L_HC_CTX_EXEC", "#L_HC_STALE"]) assert.match(locale, new RegExp(`"${key}":`), `${lang} ${key}`);
  }
});

test("вид «Конфигов»: токены темы вместо rgba, отступы 4 px, кольцо фокуса, значки скрыты, стекло", () => {
  const root = path.join(__dirname, "..", "src", "renderer");
  const vue = (name) => fs.readFileSync(path.join(root, "components", "Launcher", name), "utf8");
  const panel = vue("GameConfigsPanel.vue");
  const modal = vue("GameSettingsModal.vue");
  const css = fs.readFileSync(path.join(root, "assets", "css", "launcher.css"), "utf8");
  for (const token of ["--l-chip-bg", "--l-chip-hover", "--l-note-bg", "--l-field-bg"]) assert.ok(css.includes(`${token}:`), token);
  assert.doesNotMatch(panel, /rgba\(0, 0, 0, 0\.(28|32)\)|#ff6b78/);
  // Переключатель категории: фон из токена, видимая подпись рядом с группой (aria-label остаётся).
  const check = vue("GameCfgCheck.vue");
  assert.match(check, /\.hc__cat \{[^}]*background: var\(--l-chip-bg\);/);
  assert.doesNotMatch(check.match(/\.hc__cat(-item|-input)? \{[^}]*\}/g).join("\n"), /rgba\(/);
  assert.match(check, /class="hc__cat-label"[^>]*>\{\{ \$t\("#L_HC_CATEGORY_LABEL"\) \}\}<\/span>\s*<div class="hc__cat" role="radiogroup" :aria-label/);
  assert.match(panel, /var\(--l-note-bg\)/);
  assert.match(modal, /background: var\(--l-field-bg\)/);
  assert.doesNotMatch(modal, /&__note\.is-error \{ color: #/);
  assert.match(panel, /padding: 8px 12px;\s*border-radius: var\(--l-radius-sm\);\s*background: var\(--l-note-bg\)/);
  assert.match(modal, /padding: 12px 4px 16px; scroll-padding: 4px;/);
  assert.match(modal, /padding: 16px 0 0;/);
  assert.match(panel, /&__game[\s\S]*?&:focus-visible \{ outline: 2px solid var\(--l-accent\)/);
  for (const file of ["GameSettingsModal.vue", "GameConfigsPanel.vue", "GameConfigEditor.vue", "GameCfgCheck.vue"]) {
    const lines = vue(file).split("\n").filter((line) => /<i[ >]/.test(line));
    assert.ok(lines.length && lines.every((line) => line.includes('aria-hidden="true"')), file);
  }
  assert.match(css, /\[data-theme="liquid-glass"\] :is\(\.game-settings__tab:not\(\.is-active\), \.gx-nav__item:not\(\.active\)\) \{\s*color: #e3e8ee;/);
});

test("окно во вкладке знает игру по id доставки: проверка запрашивается без сравнения с hl1-won", async () => {
  const modal = fs.readFileSync(path.join(__dirname, "..", "src", "renderer", "components", "Launcher", "GameSettingsModal.vue"), "utf8");
  assert.doesNotMatch(modal.slice(modal.indexOf("async loadRuleCheck()"), modal.indexOf("async leaveCfgFiles()")), /hl1-won|ghosting-renewed/);
  const body = modal.slice(modal.indexOf("async loadRuleCheck()"), modal.indexOf("// Уход из «Файлов cfg»"));
  const sent = [];
  const ipcRenderer = { invoke: async (channel, id, category) => { sent.push([channel, id, category]); return { status: "ok", stats: { errors: 1, warnings: 0, infos: 0 }, findings: [] }; } };
  const methods = new Function("ipcRenderer", `return ({ ${body} });`)(ipcRenderer);
  const vm = { gameId: "70", ruleCategory: "scripted", ruleCheck: null, ruleCheckLoading: false, ruleCheckFailed: false };
  await methods.loadRuleCheck.call(vm);
  assert.deepEqual(sent, [["player-config-check", "70", "scripted"]]);
  assert.equal(vm.ruleCheck.status, "ok");
  // Чужая игра: main отвечает unsupported, раздела нет.
  ipcRenderer.invoke = async () => ({ status: "unsupported" });
  await methods.loadRuleCheck.call(vm);
  assert.equal(vm.ruleCheck, null);
  assert.equal(vm.ruleCheckFailed, false);
});

test("проверка: одна колонка и разделитель шагов пути, ключ bind в кавычках", () => {
  const root = path.join(__dirname, "..", "src", "renderer");
  const check = fs.readFileSync(path.join(root, "components", "Launcher", "GameCfgCheck.vue"), "utf8");
  assert.doesNotMatch(check, /class="gx-fields/);
  assert.match(check, /\.hc__body \{[^}]*overflow-y: auto/);
  assert.ok(check.includes('.join(" › ")'));
  for (const lang of ["ru", "en"]) {
    assert.ok(fs.readFileSync(path.join(root, "utils", "locales", `${lang}.locale.js`), "utf8").includes('"#L_HC_CTX_BIND": "bind \\"%0%\\""'), lang);
  }
});