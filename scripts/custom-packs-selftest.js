"use strict";

// Моды игр в персонализации (src/main/game-delivery/custom-packs.js): HL1 —
// копии поверх valve_WON, Ghosting — папка мода в ghostingmod/custom/<id>.
// Включение и выключение, сбой посреди копирования, доступность по наличию
// файлов в игре, потерянное состояние и совместная работа с проверкой файлов.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const packs = require("../src/main/game-delivery/custom-packs");
const { matchesMutable } = require("../src/main/game-delivery/mutable-paths");
const IntegrityService = require("../src/main/game-delivery/IntegrityService");
const filesManifest = require("../src/main/game-delivery/files-manifest");
const wonFiles = require("../config/games/hl1-won-1.0.0.files.json");

const HL1 = packs.GAMES["hl1-won"];
const GH = packs.GAMES["ghosting-renewed"];
const SOURCE_DIR = HL1.sourceDir;
const TARGET_DIR = HL1.targetDir();
const BASE_DIR = HL1.baseDir;
const byId = (id) => packs.PACKS.find((pack) => pack.id === id);
const VIEWMODELS = byId("arianon-viewmodels");
const SOUNDS = byId("oxcjae-soundpack-lite");
const HUD = byId("condensed-hud");
const KNIFE = byId("m9-bayonet");

const temps = [];
const tempDir = (prefix) => { const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix)); temps.push(dir); return dir; };
test.after(() => temps.forEach((dir) => fs.rmSync(dir, { recursive: true, force: true })));

const sha = (buffer) => crypto.createHash("sha256").update(buffer).digest("hex");
const at = (root, relative) => path.join(root, ...relative.split("/"));

function write(root, relative, text) {
  fs.mkdirSync(path.dirname(at(root, relative)), { recursive: true });
  fs.writeFileSync(at(root, relative), text);
}

// Папка игры: оригиналы и файлы всех модов игры в hlsr-packs (как после патча).
function makeGame(options = {}) {
  const root = tempDir("hlsr-cpk-");
  write(root, "Half-Life/hl.exe", "exe");
  write(root, "ghostingmod/scripts/HudLayout.res", "stock layout");
  for (const pack of packs.PACKS) {
    const game = packs.GAMES[pack.game];
    for (const file of pack.files) {
      if (game.baseDir) write(root, `${game.baseDir}/${file}`, `original ${file}`);
      if (options.sources !== false) write(root, `${game.sourceDir}/${pack.id}/${file}`, `${pack.id} ${file}`);
    }
  }
  return root;
}

// Всё дерево папки: путь → sha256 (папки — пометкой).
function tree(root) {
  const out = {};
  const walk = (dir, rel) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const next = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        out[next + "/"] = "dir";
        walk(path.join(dir, entry.name), next);
      } else out[next] = sha(fs.readFileSync(path.join(dir, entry.name)));
    }
  };
  walk(root, "");
  return out;
}

// Дерево без служебной папки модов: так игра выглядит для движка.
const gameTree = (root) => Object.fromEntries(Object.entries(tree(root)).filter(([key]) => !key.startsWith(".hlsr")));

const enabledOf = (root, game = "hl1-won") => Object.fromEntries(packs.list(root, game).map((pack) => [pack.id, pack.enabled]));

test("каталог HL1: файлы есть в WON, пути строчные, паки не пересекаются", () => {
  const won = new Set(wonFiles.files.map((file) => file.path));
  const seen = new Set();
  for (const target of packs.targets()) {
    assert.ok(!seen.has(target.toLowerCase()), `${target} в двух модах`);
    seen.add(target.toLowerCase());
    if (!target.startsWith(`${TARGET_DIR}/`)) continue;
    assert.equal(target, target.toLowerCase().replace(/^half-life\/valve_won/, "Half-Life/valve_WON"));
    const relative = target.slice(TARGET_DIR.length + 1);
    assert.ok(won.has(`${BASE_DIR}/${relative}`), `в WON нет оригинала ${relative}`);
    assert.ok(!won.has(target), `${target} — файл сборки, его нельзя перекрывать`);
  }
  assert.deepEqual(packs.list(null, "hl1-won").map((pack) => pack.files.length), [15, 23, 76]);
});

test("каталог Ghosting: мод целиком в ghostingmod/custom/<id>, нож без мировой модели", () => {
  assert.deepEqual(packs.list(null, "ghosting-renewed").map((pack) => [pack.id, pack.files.length]), [["condensed-hud", 64], ["m9-bayonet", 10], ["signed-smg", 1], ["signed-shotgun", 1], ["signed-rpg", 1]]);
  const ghosting = packs.PACKS.filter((pack) => pack.game === "ghosting-renewed");
  for (const pack of ghosting) assert.equal(GH.targetDir(pack), `ghostingmod/custom/${pack.id}`);
  // Все папки custom/* монтируются разом: один файл в двух модах — победит тот, что раньше по алфавиту.
  const relative = ghosting.flatMap((pack) => pack.files.map((file) => file.toLowerCase()));
  assert.equal(new Set(relative).size, relative.length, "моды Ghosting не перекрывают файлы друг друга");
  // Перекраски Eold — только текстура оружия в руках под именем из hl2_textures_dir.vpk.
  assert.deepEqual(ghosting.filter((pack) => pack.kind === "skins").map((pack) => [pack.id, pack.author, pack.files]), [
    ["signed-smg", "Eold", ["materials/models/weapons/v_smg1/v_smg1_sheet.vtf"]],
    ["signed-shotgun", "Eold", ["materials/models/weapons/v_shotgun/vshotgun_albedo.vtf"]],
    ["signed-rpg", "Eold", ["materials/models/weapons/v_rocket_launcher/rocket launcher sheet.vtf"]],
  ]);
  assert.ok(!KNIFE.files.some((file) => /w_crowbar/i.test(file)), "w_crowbar меняет зону подбора лома");
  for (const file of ["scripts/hudlayout.res", "scripts/hudanimations.txt", "resource/clientscheme.res", "resource/valve_russian.txt"])
    assert.ok(HUD.files.includes(file), file);
  assert.ok(!HUD.files.some((file) => /sound\.cache$/i.test(file)), "кэш движка не входит в мод");
  assert.equal(packs.hasPacks("ghosting-renewed"), true);
  assert.equal(packs.hasPacks("hl2-4260"), false);
});

test("проверка файлов: копии модов — слой кастомизации, сами моды в hlsr-packs проверяются", () => {
  for (const target of packs.targets()) assert.ok(matchesMutable(target), target);
  for (const pack of packs.PACKS)
    for (const file of pack.files) assert.ok(!matchesMutable(`${packs.GAMES[pack.game].sourceDir}/${pack.id}/${file}`), file);
  assert.ok(!matchesMutable("Half-Life/valve_WON/models/v_chub.mdl"), "чужие модели в valve_WON остаются лишними");
});

test("доступность: без файлов мода переключатель выключен, включить нельзя", () => {
  const root = makeGame({ sources: false });
  assert.deepEqual(packs.list(root, "hl1-won").map((pack) => pack.available), [false, false, false]);
  assert.deepEqual(packs.list(null, "ghosting-renewed").map((pack) => pack.available), [false, false, false, false, false]);
  assert.throws(() => packs.setEnabled(root, VIEWMODELS.id, true), { code: "CUSTOM_PACK_NOT_AVAILABLE" });
  assert.throws(() => packs.setEnabled(root, HUD.id, true), { code: "CUSTOM_PACK_NOT_AVAILABLE" });
  assert.ok(!fs.existsSync(at(root, packs.STATE_DIR)));

  const full = makeGame();
  assert.deepEqual(packs.list(full, "hl1-won").map((pack) => pack.available), [true, true, true]);
  fs.rmSync(at(full, `${SOURCE_DIR}/${SOUNDS.id}/${SOUNDS.files[40]}`));
  assert.deepEqual(packs.list(full, "hl1-won").map((pack) => pack.available), [true, true, false], "одного файла не хватает");
  assert.throws(() => packs.setEnabled(full, "nope", true), { code: "CUSTOM_PACK_UNKNOWN" });
});

test("HL1: включение кладёт копии в valve_WON, valve не трогает; выключение возвращает как было", () => {
  const root = makeGame();
  // Свой файл игрока на месте одной из копий: уходит в резерв и возвращается.
  write(root, `${TARGET_DIR}/models/v_crowbar.mdl`, "my crowbar");
  const before = gameTree(root);

  packs.setEnabled(root, VIEWMODELS.id, true);
  packs.setEnabled(root, SOUNDS.id, true);
  assert.deepEqual(enabledOf(root), { "arianon-viewmodels": true, "item-sprites": false, "oxcjae-soundpack-lite": true });
  for (const pack of [VIEWMODELS, SOUNDS])
    for (const file of pack.files) {
      assert.equal(fs.readFileSync(at(root, `${TARGET_DIR}/${file}`), "utf8"), `${pack.id} ${file}`);
      assert.equal(fs.readFileSync(at(root, `${BASE_DIR}/${file}`), "utf8"), `original ${file}`);
    }

  packs.setEnabled(root, VIEWMODELS.id, true); // повторное включение ничего не меняет
  packs.setEnabled(root, VIEWMODELS.id, false);
  packs.setEnabled(root, SOUNDS.id, false);
  assert.deepEqual(gameTree(root), before, "копии убраны, свой файл игрока на месте, пустых папок нет");
  assert.equal(fs.readdirSync(at(root, packs.STATE_DIR)).join(), "state.json", "резервных копий не осталось");
});

test("Ghosting: мод ложится папкой в ghostingmod/custom/<id>, файлы игры не меняются", () => {
  const root = makeGame();
  const before = gameTree(root);
  packs.setEnabled(root, HUD.id, true);
  packs.setEnabled(root, KNIFE.id, true);
  assert.deepEqual(enabledOf(root, "ghosting-renewed"), { "condensed-hud": true, "m9-bayonet": true, "signed-smg": false, "signed-shotgun": false, "signed-rpg": false });
  assert.deepEqual(fs.readdirSync(at(root, "ghostingmod/custom")).sort(), ["condensed-hud", "m9-bayonet"]);
  const changed = Object.keys(gameTree(root)).filter((key) => before[key] === undefined);
  assert.ok(changed.every((key) => key.startsWith("ghostingmod/custom/")), "новое — только в custom");
  assert.equal(fs.readFileSync(at(root, "ghostingmod/custom/condensed-hud/scripts/hudlayout.res"), "utf8"), "condensed-hud scripts/hudlayout.res");
  assert.equal(fs.readFileSync(at(root, "ghostingmod/scripts/HudLayout.res"), "utf8"), "stock layout");

  packs.setEnabled(root, HUD.id, false);
  packs.setEnabled(root, KNIFE.id, false);
  assert.deepEqual(gameTree(root), before, "папки модов и пустая custom убраны");
});

test("состояние потеряно (переустановка переносит ghostingmod/custom, но не .hlsr): мод виден включённым и выключается", () => {
  const root = makeGame();
  const before = gameTree(root);
  packs.setEnabled(root, HUD.id, true);
  packs.setEnabled(root, VIEWMODELS.id, true);
  fs.rmSync(at(root, packs.STATE_DIR), { recursive: true });

  assert.equal(enabledOf(root, "ghosting-renewed")[HUD.id], true);
  assert.equal(enabledOf(root)[VIEWMODELS.id], true);
  // Копия, которую поменяли, — уже не мод: такой пак не считается включённым.
  write(root, `${TARGET_DIR}/models/v_357.mdl`, "edited");
  assert.equal(enabledOf(root)[VIEWMODELS.id], false);
  write(root, `${TARGET_DIR}/models/v_357.mdl`, `${VIEWMODELS.id} models/v_357.mdl`);

  packs.setEnabled(root, HUD.id, false);
  packs.setEnabled(root, VIEWMODELS.id, false);
  assert.deepEqual(gameTree(root), before);
});

test("файлы мода «только чтение» (так их ставит патч): включение и выключение проходят, копии записываемые", () => {
  const root = makeGame();
  for (const file of SOUNDS.files) fs.chmodSync(at(root, `${SOURCE_DIR}/${SOUNDS.id}/${file}`), 0o444);
  const mine = SOUNDS.files[0];
  write(root, `${TARGET_DIR}/${mine}`, "my sound");
  fs.chmodSync(at(root, `${TARGET_DIR}/${mine}`), 0o444);
  const before = gameTree(root);

  try {
    packs.setEnabled(root, SOUNDS.id, true);
    // На Windows атрибут переносится с копией; Linux меняет режим только у своих файлов
    if (process.platform === "win32")
      for (const file of SOUNDS.files) assert.ok(fs.statSync(at(root, `${TARGET_DIR}/${file}`)).mode & 0o200, file);
    packs.setEnabled(root, SOUNDS.id, false);
    assert.deepEqual(gameTree(root), before);
    assert.deepEqual(enabledOf(root)[SOUNDS.id], false);
  } finally {
    // rmSync в Node из Electron не удаляет папку с такими файлами
    for (const file of SOUNDS.files) fs.chmodSync(at(root, `${SOURCE_DIR}/${SOUNDS.id}/${file}`), 0o666);
    fs.chmodSync(at(root, `${TARGET_DIR}/${mine}`), 0o666);
  }
});

test("HL1: выключение не трогает файл, который поменяли после включения", () => {
  const root = makeGame();
  packs.setEnabled(root, VIEWMODELS.id, true);
  write(root, `${TARGET_DIR}/models/v_357.mdl`, "edited later");
  packs.setEnabled(root, VIEWMODELS.id, false);
  assert.equal(fs.readFileSync(at(root, `${TARGET_DIR}/models/v_357.mdl`), "utf8"), "edited later");
  assert.ok(!fs.existsSync(at(root, `${TARGET_DIR}/models/v_crowbar.mdl`)));
  assert.equal(enabledOf(root)[VIEWMODELS.id], false);
});

// Снимок папки после 1-го, 8-го и последнего файла: «питание пропало здесь».
// Восстановление на каждом снимке должно привести к целому состоянию.
function snapshots(root, total, run) {
  const shots = [];
  let index = 0;
  run({
    afterFile() {
      if (![1, 8, total].includes(++index)) return;
      const copy = tempDir("hlsr-cpk-shot-");
      fs.cpSync(root, copy, { recursive: true });
      shots.push(copy);
    },
  });
  return shots;
}

test("сбой посреди включения: следующий вызов откатывает до прежнего вида", () => {
  for (const pack of [VIEWMODELS, KNIFE]) {
    const root = makeGame();
    if (pack === VIEWMODELS) write(root, `${TARGET_DIR}/models/v_gauss.mdl`, "my gauss");
    const before = gameTree(root);

    const shots = snapshots(root, pack.files.length, (hooks) => packs.setEnabled(root, pack.id, true, { hooks }));
    // Последний снимок — после всех копий, но до записи «включено»: тоже откат.
    assert.equal(shots.length, 3);
    for (const shot of shots) {
      assert.notDeepEqual(gameTree(shot), before, "в снимке уже часть копий");
      assert.equal(packs.recover(shot), true);
      assert.deepEqual(gameTree(shot), before, pack.id);
      assert.equal(enabledOf(shot, pack.game)[pack.id], false);
      assert.equal(packs.recover(shot), false, "второй раз делать нечего");
    }
  }
});

test("сбой посреди выключения: следующий вызов доводит выключение до конца", () => {
  for (const pack of [VIEWMODELS, KNIFE]) {
    const root = makeGame();
    if (pack === VIEWMODELS) write(root, `${TARGET_DIR}/models/v_rpg.mdl`, "my rpg");
    const before = gameTree(root);
    packs.setEnabled(root, pack.id, true);

    const shots = snapshots(root, pack.files.length, (hooks) => packs.setEnabled(root, pack.id, false, { hooks }));
    for (const shot of shots) {
      // Список с recover сам доделывает операцию (так его зовёт вкладка при закрытой игре).
      assert.equal(packs.list(shot, pack.game, { recover: true }).find((item) => item.id === pack.id).enabled, false);
      assert.deepEqual(gameTree(shot), before, pack.id);
    }
    assert.deepEqual(gameTree(root), before);
  }
});

test("ошибка посреди включения откатывается сразу", () => {
  const root = makeGame();
  const before = gameTree(root);
  let count = 0;
  const hooks = { afterFile() { if (++count === 5) throw Object.assign(new Error("диск полон"), { code: "ENOSPC" }); } };
  assert.throws(() => packs.setEnabled(root, SOUNDS.id, true, { hooks }), { code: "ENOSPC" });
  assert.deepEqual(gameTree(root), before);
  assert.equal(enabledOf(root)[SOUNDS.id], false);
  assert.equal(packs.recover(root), false, "журнал закрыт");
});

test("проверка файлов не видит копий включённых модов и проверяет сами моды", async () => {
  const root = makeGame();
  const entries = Object.entries(tree(root)).filter(([key, value]) => value !== "dir").map(([relative]) => {
    const bytes = fs.readFileSync(at(root, relative));
    return { path: relative, size: bytes.length, sha256: sha(bytes) };
  });
  const manifest = { version: "1.0.1", files: entries };
  const check = () => new IntegrityService().check({ installPath: root, gameId: "hl1-won", filesManifest: manifest, preservePaths: [] });

  for (const pack of [VIEWMODELS, SOUNDS, HUD, KNIFE]) packs.setEnabled(root, pack.id, true);
  let result = await check();
  assert.equal(result.ok, true);
  assert.deepEqual(result.extra, [], "копии в valve_WON и ghostingmod/custom — не лишние файлы");

  // Испорченный файл мода — повреждение сборки, его чинит «Восстановить», копия при этом не трогается.
  write(root, `${SOURCE_DIR}/${SOUNDS.id}/${SOUNDS.files[0]}`, "broken");
  write(root, `${GH.sourceDir}/${HUD.id}/${HUD.files[0]}`, "broken");
  result = await check();
  assert.equal(result.ok, false);
  assert.deepEqual(result.sizeMismatch.map((item) => item.path).sort(),
    [`${GH.sourceDir}/${HUD.id}/${HUD.files[0]}`, `${SOURCE_DIR}/${SOUNDS.id}/${SOUNDS.files[0]}`].sort());

  // Манифест выпуска собирается из дерева без копий, даже если моды включены.
  const collected = filesManifest.collectFiles(root).files.map((file) => file.relative);
  assert.ok(collected.includes(`${SOURCE_DIR}/${VIEWMODELS.id}/models/v_357.mdl`));
  assert.ok(collected.includes(`${GH.sourceDir}/${KNIFE.id}/models/weapons/v_crowbar.mdl`));
  assert.ok(!collected.includes(`${TARGET_DIR}/models/v_357.mdl`));
  assert.ok(!collected.some((file) => file.startsWith("ghostingmod/custom/")));
});

// --- вкладка в лаунчере (CustomPacks.vue): разбор исходников, как в launcher-ui-selftest ---

const RENDERER = path.join(__dirname, "../src/renderer");
const readRenderer = (relative) => fs.readFileSync(path.join(RENDERER, relative), "utf8");
const localeKeys = (lang) => {
  const keys = new Map();
  for (const match of readRenderer(`utils/locales/${lang}.locale.js`).matchAll(/^\s*"(#L_[A-Z0-9_]+)": ("(?:[^"\\]|\\.)*"),$/gm))
    keys.set(match[1], JSON.parse(match[2]));
  return keys;
};

test("вкладка: все строки есть на русском и английском, в новых строках нет тире", () => {
  const view = readRenderer("components/Launcher/CustomPacks.vue");
  const extras = readRenderer("components/Launcher/GhostingSettingsExtras.vue");
  const used = new Set([...view.matchAll(/"(#L_[A-Z0-9_]+)"/g), ...extras.matchAll(/'(#L_CPK_[A-Z0-9_]+)'/g)].map((m) => m[1]).filter((name) => !name.endsWith("_")));
  const key = (pack) => pack.id.replace(/-/g, "_").toUpperCase();
  for (const pack of packs.PACKS) {
    used.add(`#L_CPK_TITLE_${key(pack)}`);
    used.add(`#L_CPK_DESC_${key(pack)}`);
    used.add(`#L_CPK_KIND_${pack.kind.toUpperCase()}`);
  }
  for (const lang of ["ru", "en"]) {
    const keys = localeKeys(lang);
    for (const name of used) assert.ok(keys.has(name), `${name} нет в ${lang}`);
    for (const [name, text] of keys) if (name.startsWith("#L_CPK_")) assert.ok(!/[—–]/.test(text), `тире в ${name} (${lang})`);
  }
  assert.ok(extras.includes("'#L_CPK_TAB_HL1'") && extras.includes("'#L_CPK_TAB_GHOSTING'"), "вкладка у обеих игр");
});

test("вкладка: превью — кнопка-переключатель с фокусом, выбранный мод подсвечен цветом темы", () => {
  const view = readRenderer("components/Launcher/CustomPacks.vue");
  const preview = /<button\s[^>]*class="cpk-card__preview"[^>]*>/s.exec(view);
  assert.ok(preview, "превью — <button>");
  for (const attr of ['type="button"', ":aria-pressed=", ":aria-label=", ':disabled="!pack.available || !!switching"', '@click="toggle(pack, !pack.enabled)"'])
    assert.ok(preview[0].includes(attr), attr);
  assert.ok(view.includes('@change="toggle(pack, $event.target.checked, $event.target)"'), "переключатель остался");
  const style = view.slice(view.indexOf("<style"));
  assert.match(style, /&__preview \{[^}]*cursor: pointer;/s);
  assert.match(style, /&:focus-visible \{ outline: 2px solid/);
  // Подсветка — от --l-text темы: белая в тёмных темах, тёплая светлая в «Бумаге».
  assert.match(style, /&\.is-on &__preview \{\s*box-shadow:\s*0 0 0 2px var\(--l-text\)/);
  assert.match(style, /border-radius: 8px;/, "скруглённые снимки");
  for (const match of view.matchAll(/require\("@\/assets\/([^"]+)"\)/g))
    assert.ok(fs.existsSync(path.join(RENDERER, "assets", match[1])), match[1]);
});

test("вкладка: плитки заполняют панель, в узком окне — одна колонка, список файлов не раздувает сетку", () => {
  const view = readRenderer("components/Launcher/CustomPacks.vue");
  const style = view.slice(view.indexOf("<style"));
  // Сетка: колонок сколько влезает, ряды не ниже 380 px и растягиваются на высоту панели.
  assert.match(style, /\.cpk__grid \{[^}]*grid-template-columns: repeat\(auto-fit, minmax\(min\(100%, 320px\), 1fr\)\);[^}]*grid-auto-rows: minmax\(380px, auto\);[^}]*align-content: stretch;/s);
  assert.ok(view.includes('<div class="cpk__grid">'), "плитки в сетке");
  // Превью растёт на свободное место плитки; кадры рядом или друг под другом по пропорциям сцены.
  assert.match(style, /&__preview \{[^}]*flex: 1 1 auto;[^}]*min-height: 150px;/s);
  assert.match(style, /container: cpk-stage \/ size;/);
  assert.match(style, /@container cpk-stage \(min-aspect-ratio: 2\/1\) \{\s*\.cpk-card__shots \{ flex-direction: row; \}/);
  // Три пака HL1 в две колонки: третий по центру.
  assert.match(style, /@container cpk \(min-width: 654px\) and \(max-width: 987px\) \{\s*\.cpk-card:last-child:nth-child\(odd\) \{\s*grid-column: 1 \/ -1;\s*justify-self: center;/);
  // Узкое окно: одна колонка, плитка в строку; правила стоят после правил плитки, иначе их перебьют.
  const narrow = style.indexOf("@container cpk (min-width: 460px) and (max-width: 653px)");
  assert.ok(narrow > style.indexOf("\n.cpk-card {") && narrow > style.indexOf("\n.cpk-shot {"), "правила узкого окна — после правил плитки");
  assert.match(style.slice(narrow), /\.cpk-card \{ flex-direction: row; \}/);
  // Список файлов прокручивается внутри плитки.
  assert.match(style, /&__list \{[^}]*max-height: 200px;[^}]*overflow-y: auto;/s);
  // Пак без снимков: крупный значок на подложке цвета акцента, размер от сцены.
  assert.match(style, /&__icon \{[^}]*font-size: clamp\(40px, 34cqmin, 128px\);/s);
  assert.match(style, /&__icon \{[^}]*color-mix\(in srgb, var\(--accent/s);
  // У каждого снимка задана точка, которая остаётся в кадре при обрезке.
  const shots = [...view.matchAll(/require\("@\/assets\/launcher\/custom-packs\/[^"]+"\)(?:, label: "[^"]+")?(, focus: "[^"]+")?/g)];
  assert.equal(shots.length, 8);
  // У ножа один кадр без подписи «Лом/Нож»: нож на фоне неба.
  assert.match(view, /"m9-bayonet": \[\s*\{ src: require\("@\/assets\/launcher\/custom-packs\/m9-bayonet\.jpg"\), focus: "[^"]+" \},\s*\]/);
  assert.match(view, /<span v-if="shot\.label" class="cpk-shot__label">/);
  assert.ok(shots.every((match) => match[1]), "focus у каждого снимка");
});

test("предпросмотр HUD: при включённом Condensed HUD кадр рисует раскладку мода и его шрифт цифр", () => {
  const renderer = readRenderer("components/Launcher/ghosting-overlay-renderer.js");
  const preview = readRenderer("components/Launcher/GhostingSettingsPreview.vue");
  const extras = readRenderer("components/Launcher/GhostingSettingsExtras.vue");
  // Раскладка мода — все блоки стандартного HUD, кроме таймера; альфа FgColor мода — 220.
  const condensed = renderer.slice(renderer.indexOf("const CONDENSED = ["), renderer.indexOf("];", renderer.indexOf("const CONDENSED = [")));
  assert.deepEqual([...condensed.matchAll(/\{ id: '([a-z0-9]+)'/g)].map((m) => m[1]), ["health", "suit", "speed", "ammo", "ammo2"]);
  assert.match(renderer, /const CONDENSED_ALPHA = 220;/);
  assert.match(renderer, /assets\.hudMod === 'condensed-hud' \? this\.condensedMasks\(assets\.hudModFont \|\| ''\) : this/);
  // Ячейка цифр — по win-метрикам шрифта мода, запасной шрифт встаёт в неё же.
  assert.match(renderer, /tall \* 2048 \/ 2387/);
  // Шрифт цифр читается из файлов мода, путь — из каталога пака.
  const font = /value: '(resource\/[^']+\.ttf)'/.exec(preview);
  assert.ok(font && HUD.files.includes(font[1]), "шрифт мода из каталога пака");
  assert.match(preview, /hudMod: \{ type: String, default: '' \}/);
  assert.match(extras, /:hud-mod="hudMod"/);
  assert.match(extras, /pack\.id === 'condensed-hud' && pack\.enabled/);
  assert.match(extras, /@packs="modPacks = \$event"/);
  // Карточка больше не говорит, что предпросмотр показывает обычный HUD.
  for (const lang of ["ru", "en"]) assert.ok(!/предпросмотр|preview/i.test(localeKeys(lang).get("#L_CPK_DESC_CONDENSED_HUD")), lang);
});

test("предпросмотр HUD: шрифт мода находится в ghostingmod/custom/condensed-hud только после включения", () => {
  const ghostingAssets = require("../src/main/game-delivery/ghosting-assets");
  const root = makeGame();
  // Наименьший TTF, который пропускает проверка: заголовок и одна пустая таблица.
  const ttf = Buffer.alloc(28); ttf.writeUInt32BE(0x00010000, 0); ttf.writeUInt16BE(1, 4); ttf.writeUInt32BE(28, 20);
  fs.writeFileSync(at(root, `${GH.sourceDir}/${HUD.id}/resource/din1451alt2.ttf`), ttf);
  assert.throws(() => ghostingAssets.readPreviewAsset(root, "font", "resource/din1451alt2.ttf"), { code: "ASSET_READ_FAILED" });
  packs.setEnabled(root, HUD.id, true);
  const result = ghostingAssets.readPreviewAsset(root, "font", "resource/din1451alt2.ttf");
  assert.equal(result.dataUrl, `data:font/ttf;base64,${ttf.toString("base64")}`);
});

test("код настроек: моды с диска входят в код, при импорте переключаются сразу, ошибки названы", () => {
  const main = fs.readFileSync(path.join(__dirname, "..", "src", "main", "game-delivery", "index.js"), "utf8");
  const handler = main.slice(main.indexOf("  personalizationCode(action, id, value) {"), main.indexOf("  gameSettings(action, id, settings, scope, options) {"));
  assert.match(handler, /mods: Object\.fromEntries\(customPacks\.list\(installPath, gameId\)/, "экспорт берёт моды с диска");
  assert.match(handler, /const \{ mods, \.\.\.look \} = personalizationCode\.decode/, "моды не попадают в черновик");
  const view = readRenderer("components/Launcher/GamePersonalizationCode.vue");
  assert.match(view, /invoke\("custom-packs-set", this\.gameId, pack\.id, want\)/);
  assert.match(view, /if \(want && !pack\.available\) \{ missing\.push/, "моды без файлов не включаются, а называются");
  assert.match(view, /import \{ ERRORS as PACK_ERRORS \} from "\.\/CustomPacks"/, "причины те же, что на вкладке модов");
  assert.match(readRenderer("components/Launcher/GamePersonalization.vue"), /this\.\$refs\.extras\.modPacks = packs/, "предпросмотр HUD узнаёт о модах из кода");
  const used = [...view.matchAll(/"(#L_PZ_CODE_MODS?_[A-Z_]+)"/g)].map((m) => m[1]);
  assert.equal(new Set(used).size, 6);
  for (const lang of ["ru", "en"]) {
    const keys = localeKeys(lang);
    for (const name of used) {
      assert.ok(keys.has(name), `${name} нет в ${lang}`);
      assert.ok(!/[—–]/.test(keys.get(name)), `тире в ${name} (${lang})`);
    }
  }
});

test("прослушивание: оригинал из valve и замена из пака, другие пути не читаются", () => {
  const root = makeGame();
  const file = SOUNDS.files[3];
  const decode = (value) => Buffer.from(value.dataUrl.replace(/^data:audio\/wav;base64,/, ""), "base64").toString();
  assert.equal(decode(packs.readSound(root, SOUNDS.id, file, "original")), `original ${file}`);
  assert.equal(decode(packs.readSound(root, SOUNDS.id, file, "pack")), `${SOUNDS.id} ${file}`);
  assert.throws(() => packs.readSound(root, SOUNDS.id, "../../hl.exe", "pack"), { code: "CUSTOM_PACK_FILE_UNKNOWN" });
  assert.throws(() => packs.readSound(root, SOUNDS.id, file, "other"), { code: "CUSTOM_PACK_FILE_UNKNOWN" });
  assert.throws(() => packs.readSound(root, VIEWMODELS.id, VIEWMODELS.files[0], "pack"), { code: "CUSTOM_PACK_FILE_UNKNOWN" });
  assert.throws(() => packs.readSound(root, HUD.id, HUD.files[0], "pack"), { code: "CUSTOM_PACK_FILE_UNKNOWN" });
});
