"use strict";

// Самотест чистой логики страницы «Демо» (src/renderer/components/Launcher/demo-tool.js)
// и её текстов: время демо из времени клипа, допустимые сочетания, коды ошибок.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const path = require("path");

const demo = require("../src/renderer/components/Launcher/demo-tool");
const locales = require("../src/renderer/components/Launcher/demo-locales");

test("formatClock и parseClock", () => {
  assert.equal(demo.formatClock(0), "0:00.0");
  assert.equal(demo.formatClock(65.34), "1:05.3");
  assert.equal(demo.formatClock(59.96), "1:00.0");
  assert.equal(demo.formatClock(3725.5), "1:02:05.5");
  assert.equal(demo.formatClock(144.46, 0), "2:24");
  assert.equal(demo.parseClock("1:05.3"), 65.3);
  assert.equal(demo.parseClock("65,3"), 65.3);
  assert.equal(demo.parseClock("1:02:05"), 3725);
  assert.equal(demo.parseClock(" 12 "), 12);
  for (const bad of ["", "abc", "1:75", "1.5:10", "-3", "1:2:3:4", null]) assert.equal(demo.parseClock(bad), null, String(bad));
});

test("время демо из времени клипа: интервал, скорость, вырезанные загрузки", () => {
  const video = { range: { from: 2, to: 10 }, speed: 2, duration: 4 };
  // Без меток: от + время × скорость.
  assert.equal(demo.demoTimeAt(0, video, []), 2);
  assert.equal(demo.demoTimeAt(1.5, video, []), 5);
  assert.equal(demo.demoTimeAt(99, video, []), 10, "не дальше конца интервала");
  // Метка после вырезанной загрузки: 3 с демо выпали, метка в видео на 1 с.
  const markers = [{ t: 0, sourceTime: 2 }, { t: 1, sourceTime: 7 }];
  assert.equal(demo.demoTimeAt(0.5, video, markers), 3);
  assert.equal(demo.demoTimeAt(1.5, video, markers), 8);
  // Обратно — для подсветки интервала на полосе.
  assert.equal(demo.videoTimeAt(8, video, markers), 1.5);
  assert.equal(demo.videoTimeAt(3, video, markers), 0.5);
  // Предпросмотр всего рана: время совпадает.
  assert.equal(demo.demoTimeAt(12.25, { range: { from: 0, to: 144.46 }, speed: 1 }, []), 12.25);
});

test("плеер получает видео только выбранной демки; интервал — только её время", () => {
  const gh2 = { item: { id: "gh2", duration: 144.46 }, markers: [{ t: 25.85, type: "map", label: "d1_trainstation_02" }] };
  const gh1 = { item: { id: "gh1", duration: 52.31 }, markers: [{ t: 4.36, type: "saveload", label: "2" }] };
  const media = { itemId: "gh2", result: { mediaUrl: "hlsr-media://media/x", video: { range: { from: 0, to: 20 }, speed: 1 }, markers: [{ t: 0, type: "map", sourceTime: 0 }] } };
  assert.deepEqual(demo.playerInput(media, gh2), { own: true, src: "hlsr-media://media/x", video: media.result.video, markers: media.result.markers });
  assert.deepEqual(demo.playerInput(media, gh1), { own: false, src: null, video: null, markers: gh1.markers }, "чужое видео не в плеере");
  assert.deepEqual(demo.playerInput(null, gh1).markers, gh1.markers);
  assert.deepEqual(demo.playerInput(media, null), { own: false, src: null, video: null, markers: [] });
  // Отметки прижимаются к длине своей демки.
  assert.deepEqual(demo.markRange(null, "from", 10, 52.31), { range: { from: 10, to: 52.31 }, swapped: false });
  assert.deepEqual(demo.markRange({ from: 10, to: 30 }, "to", 140, 52.31), { range: { from: 10, to: 52.31 }, swapped: false }, "время чужой, более длинной демки прижато к своей");
  // Конец раньше начала (и наоборот): обе отметки остаются, меняются местами, интерфейс говорит об этом.
  assert.deepEqual(demo.markRange({ from: 10, to: 20 }, "from", 30, 52.31), { range: { from: 20, to: 30 }, swapped: true });
  assert.deepEqual(demo.markRange({ from: 30, to: 52.31 }, "to", 5, 52.31), { range: { from: 5, to: 30 }, swapped: true });
  assert.deepEqual(demo.markRange(null, "from", 0, 52.31), { range: null, swapped: false }, "весь ран");
  assert.deepEqual(demo.markRange(null, "to", 0, 52.31), { range: null, swapped: false }, "конец в нуле не даёт пустой отрезок");
  assert.deepEqual(demo.markRange(null, "from", 52.31, 52.31), { range: null, swapped: false }, "начало в конце рана тоже");
});

test("сочетания разрешение × FPS из demo-options", () => {
  const options = {
    resolutions: [{ height: 720, width: 1280 }, { height: 1080, width: 1920 }, { height: 2160, width: 3840 }],
    capabilities: { fps: [30, 60, 120] },
    modes: {
      auto: { encoder: "h264_nvenc", combos: [
        { height: 720, fps: 30 }, { height: 720, fps: 60 }, { height: 720, fps: 120 },
        { height: 1080, fps: 30 }, { height: 1080, fps: 60 },
        { height: 2160, fps: 30 },
      ] },
      gpu: null,
      cpu: { encoder: "libx264", combos: [{ height: 720, fps: 30 }] },
    },
  };
  const auto = demo.combosFor(options, "auto");
  assert.deepEqual(demo.heightsOf(auto), [720, 1080, 2160]);
  assert.deepEqual(demo.fpsFor(auto, 1080), [30, 60]);
  assert.deepEqual(demo.pickCombo(auto, 1080, 60), { height: 1080, fps: 60 }, "умолчание 1080p60 остаётся");
  assert.deepEqual(demo.pickCombo(auto, 1080, 120), { height: 1080, fps: 60 }, "120 нет — ниже");
  assert.deepEqual(demo.pickCombo(auto, 1440, 60), { height: 1080, fps: 60 }, "1440 нет — ближайшее ниже");
  assert.deepEqual(demo.pickCombo(auto, 2160, 60), { height: 2160, fps: 30 });
  assert.deepEqual(demo.combosFor(options, "gpu"), [], "GPU нет — выбрать нечего");
  assert.equal(demo.pickCombo([], 1080, 60), null);
  // FFmpeg не готов: возможности адаптера целиком.
  const pending = demo.combosFor(Object.assign({}, options, { modes: null }), "auto");
  assert.equal(pending.length, 9);
});

test("группы списка: HL1, Ghosting, другие игры, добавленные", () => {
  const groups = demo.groupItems([
    { id: "m", source: "manual", gameId: null, mtimeMs: 5 },
    { id: "g1", source: "game", gameId: "ghosting-renewed", mtimeMs: 1 },
    { id: "h1", source: "game", gameId: "hl1-won", mtimeMs: 1 },
    { id: "h2", source: "game", gameId: "hl1-won", mtimeMs: 9 },
    { id: "x", source: "game", gameId: "hl2-4260", mtimeMs: 1 },
  ]);
  assert.deepEqual(groups.map((group) => group.id), ["hl1-won", "ghosting-renewed", "hl2-4260", "manual"]);
  assert.deepEqual(groups[0].items.map((item) => item.id), ["h2", "h1"], "новые сверху");
  // Неподдерживаемые (битые) — после поддерживаемых, даже если новее; выбор по умолчанию — первая поддерживаемая.
  const withBroken = demo.groupItems([
    { id: "bad", source: "game", gameId: "ghosting-renewed", supported: false, mtimeMs: 99 },
    { id: "run", source: "game", gameId: "ghosting-renewed", supported: true, mtimeMs: 1 },
  ]);
  assert.deepEqual(withBroken[0].items.map((item) => item.id), ["run", "bad"]);
  assert.equal(demo.firstPick(withBroken), "run");
  assert.equal(demo.firstPick(demo.groupItems([{ id: "only", source: "manual", supported: false }])), "only", "нет поддерживаемых — первая");
  assert.equal(demo.firstPick([]), null);
});

test("ошибки: коды контракта дают тексты, у каждого текста ru и en", () => {
  assert.deepEqual(demo.errorMessage({ code: "RENDER_NOT_READY" }), { key: "#L_DEMO_ERR_RENDER_NOT_READY", args: [] });
  assert.deepEqual(demo.errorMessage({ code: "DISCORD_TOO_LONG", recommendedMaxSeconds: 118.7, suggestedHeight: 720 }),
    { key: "#L_DEMO_ERR_DISCORD_LONG_RES", args: ["118", "720p"] });
  assert.deepEqual(demo.errorMessage({ code: "DISCORD_TOO_LONG", recommendedMaxSeconds: 40, suggestedHeight: null }),
    { key: "#L_DEMO_ERR_DISCORD_LONG", args: ["40"] });
  assert.deepEqual(demo.errorMessage({ code: "FFMPEG_FAILED", exitCode: 1 }).args, ["1"]);
  assert.deepEqual(demo.errorMessage({ code: "SOMETHING_NEW" }), { key: "#L_DEMO_ERR_GENERIC", args: ["SOMETHING_NEW"] });
  // Рендер Ghosting: подробности кода попадают в текст.
  assert.deepEqual(demo.errorMessage({ code: "RENDER_TIMEOUT", framesDone: 120, framesTotal: 8668 }).args, ["120", "8668"]);
  assert.deepEqual(demo.errorMessage({ code: "GAME_EXITED", exitCode: 3 }).args, ["3"]);
  assert.deepEqual(demo.errorMessage({ code: "SEGMENT_CHANGED", segment: 17 }), { key: "#L_DEMO_ERR_SEGMENT_CHANGED", args: ["17"] });
  // HL1: кода выхода нет (игру запускает Injector), заменённый сегмент — с именем.
  assert.deepEqual(demo.errorMessage({ code: "GAME_EXITED" }), { key: "#L_DEMO_ERR_GAME_EXITED_ANY", args: [] });
  assert.deepEqual(demo.errorMessage({ code: "SEGMENT_REPLACED", segment: 2, name: "c1a0d.dem" }),
    { key: "#L_DEMO_ERR_SEGMENT_REPLACED_NAMED", args: ["2", "c1a0d.dem"] });
  assert.deepEqual(demo.warningKeys(["KEYHUD_PARTIAL"]), ["#L_DEMO_WARN_KEYHUD_PARTIAL"]);
  for (const code of ["GOLDSRC_RUNNING", "FFMPEG_IN_GAME_DIR", "DEMO_TOO_LARGE",
    "GAME_RUNNING", "STEAM_NOT_RUNNING", "STEAM_NOT_INSTALLED", "STEAM_ELEVATED", "MAP_MISSING", "DEMO_OPEN_FAILED", "GAME_ERROR",
    "RENDER_HOST_MISSING", "RENDER_HOST_FAILED", "REGISTRY_FAILED", "AUDIO_MISSING", "AUDIO_INVALID", "GAME_NOT_INSTALLED", "DEMO_EMPTY", "RENDER_ACTIVE"])
    assert.notEqual(demo.errorMessage({ code }).key, "#L_DEMO_ERR_GENERIC", code);
  // Запуск игры во время записи: LauncherShell.notifyDeliveryError берёт #L_D_ERR_<код>.
  assert.equal(demo.ERROR_KEYS.RENDER_ACTIVE, "#L_D_ERR_RENDER_ACTIVE");

  assert.deepEqual(demo.warningKeys(["CPU_SLOW", "UNKNOWN", "CPU_SLOW"]), ["#L_DEMO_WARN_CPU_SLOW"]);
  const keys = new Set(Object.values(demo.ERROR_KEYS).concat(Object.values(demo.WARNING_KEYS),
    ["#L_DEMO_ERR_GENERIC", "#L_DEMO_ERR_DISCORD_LONG", "#L_DEMO_ERR_DISCORD_LONG_RES"]));
  for (const key of keys) for (const lang of ["ru", "en"]) assert.ok(locales[lang][key], `${key} ${lang}`);
  assert.deepEqual(Object.keys(locales.ru).sort(), Object.keys(locales.en).sort(), "одинаковые ключи ru и en");
  for (const lang of ["ru", "en"])
    for (const [key, text] of Object.entries(locales[lang])) {
      const params = (text.match(/%\d%/g) || []).sort().join();
      const other = (locales[lang === "ru" ? "en" : "ru"][key].match(/%\d%/g) || []).sort().join();
      assert.equal(params, other, `параметры ${key}`);
    }
});

// Коды берутся из текущих исходников главного процесса: новый код без текста
// валит тест, а не показывает игроку «Что-то пошло не так (КОД)».
test("у каждого кода из src/main/demo-tool есть свой текст", () => {
  const dir = path.join(__dirname, "../src/main/demo-tool");
  const codes = new Set();
  for (const file of fs.readdirSync(dir).filter((name) => name.endsWith(".js")))
    for (const match of fs.readFileSync(path.join(dir, file), "utf8").matchAll(/["']([A-Z][A-Z0-9]+_[A-Z0-9_]+)["']/g)) codes.add(match[1]);
  const skip = (code) => /^HLSR_/.test(code) || demo.WARNING_KEYS[code] || code === "DISCORD_TOO_LONG";
  const checked = [...codes].filter((code) => !skip(code));
  assert.ok(checked.length > 60, "кодов найдено: " + checked.length);
  const missing = checked.filter((code) => demo.errorMessage({ code }).key === "#L_DEMO_ERR_GENERIC");
  assert.deepEqual(missing, [], "коды без текста");
  for (const code of checked) {
    const { key } = demo.errorMessage({ code });
    assert.ok(locales.ru[key] && locales.en[key], `${code} → ${key}`);
  }
  assert.deepEqual(demo.errorMessage({ code: "RESOLUTION_LIMITED", width: 1707, height: 960 }), { key: "#L_DEMO_ERR_RESOLUTION_LIMITED", args: ["1707", "960"] });
});

test("каждый ключ #L_DEMO_ из разметки есть в текстах, тексты подключены к языкам", () => {
  const dir = path.join(__dirname, "../src/renderer/components/Launcher");
  const files = ["DemoPanel.vue", "DemoPlayer.vue", "DemoExportModal.vue", "DemoFfmpegCard.vue", "DemoViewSettings.vue", "GameSidebar.vue", "demo-tool.js"];
  const used = new Set();
  for (const file of files)
    for (const match of fs.readFileSync(path.join(dir, file), "utf8").matchAll(/#L_DEMO_[A-Z0-9_]+/g)) used.add(match[0]);
  assert.ok(used.size > 40, "ключей найдено: " + used.size);
  for (const key of used) assert.ok(locales.ru[key] && locales.en[key], key);
  for (const lang of ["ru", "en"]) {
    const text = fs.readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    assert.ok(text.includes(`...demoLocales.${lang},`), lang);
  }
});

test("оценка времени словами: секунды до минуты, дальше — минуты вверх, от часа — часы и минуты", () => {
  assert.deepEqual(demo.durationText(3), { key: "#L_DEMO_EST_SEC", args: ["5"] });
  assert.deepEqual(demo.durationText(42), { key: "#L_DEMO_EST_SEC", args: ["45"] });
  assert.deepEqual(demo.durationText(61), { key: "#L_DEMO_EST_MIN", args: ["2"] });
  assert.deepEqual(demo.durationText(1302), { key: "#L_DEMO_EST_MIN", args: ["22"] });
  assert.deepEqual(demo.durationText(3900), { key: "#L_DEMO_EST_HOUR", args: ["1", "05"] });
  for (const key of ["#L_DEMO_EST_SEC", "#L_DEMO_EST_MIN", "#L_DEMO_EST_HOUR"]) assert.ok(locales.ru[key] && locales.en[key], key);
});

test("удалённое видео: плеер «Демок» и итог экспорта узнают свой файл — по имени и папке, после удаления по пути", () => {
  const win = path.win32;
  const out = win.join("C:", "Users", "me", "Videos", "HLSR");
  const result = { file: win.join(out, "myrun2.mp4"), fileName: "myrun2.mp4" };
  assert.equal(demo.isRenderedFile(result, { name: "myrun2.mp4", outputDir: out }), true);
  assert.equal(demo.isRenderedFile(result, { name: "myrun2.mp4", outputDir: out.toLowerCase() + win.sep }), true, "регистр и слеш в конце");
  assert.equal(demo.isRenderedFile(result, { name: "myrun2.mp4", outputDir: win.join("D:", "Clips") }), false, "то же имя в другой папке");
  assert.equal(demo.isRenderedFile(result, { name: "myrun1.mp4", outputDir: out }), false);
  assert.equal(demo.isRenderedFile(result, { file: win.join(out.toLowerCase(), "MYRUN2.mp4") }), true);
  assert.equal(demo.isRenderedFile({ file: "/home/me/Videos/HLSR/a.mp4", fileName: "a.mp4" }, { name: "a.mp4", outputDir: "/home/me/Videos/HLSR" }), true, "Linux");
  assert.equal(demo.isRenderedFile({ file: null, fileName: null }, { name: "myrun2.mp4", outputDir: "x" }), false, "предпросмотр — не файл папки вывода");
  assert.equal(demo.isRenderedFile(null, { name: "myrun2.mp4" }), false);
});
