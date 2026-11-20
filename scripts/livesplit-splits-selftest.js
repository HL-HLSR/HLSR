"use strict";

// Сплиты LiveSplit по играм (src/main/game-delivery/livesplit-splits.js),
// запуск LiveSplit с ними (ToolService) и привязка оверлея LiveSplit к игре
// (livesplit-overlay.js, bindTo).

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { EventEmitter } = require("events");

const ROOT = path.resolve(__dirname, "..");
const LiveSplitSplits = require(path.join(ROOT, "src/main/game-delivery/livesplit-splits.js"));
const LiveSplitOverlay = require(path.join(ROOT, "src/main/game-delivery/livesplit-overlay.js"));
const ToolService = require(path.join(ROOT, "src/main/game-delivery/ToolService.js"));

const BASE_LAYOUT = [
  '<?xml version="1.0" encoding="UTF-8"?>',
  '<Layout version="1.6.1">',
  "  <Components>",
  "    <Component>",
  "      <Path>LiveSplit.Splits.dll</Path>",
  "    </Component>",
  "  </Components>",
  "</Layout>",
  "",
].join("\r\n");

const SETTINGS = [
  '<?xml version="1.0" encoding="UTF-8"?>',
  "<Settings>",
  "  <RecentSplits />",
  "  <ActiveAutoSplitters />",
  "</Settings>",
  "",
].join("\r\n");

function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-splits-"));
}

// Папка LiveSplit как после установки: раскладки Ghosting и settings.cfg.
function liveSplitRoot() {
  const root = tempDir();
  for (const kind of ["chapters", "maps"]) {
    fs.mkdirSync(path.join(root, "splits", kind), { recursive: true });
    fs.writeFileSync(path.join(root, "splits", kind, `${kind}.lsl`), BASE_LAYOUT);
    fs.writeFileSync(path.join(root, "splits", kind, `${kind}.lss`), "<Run />");
  }
  fs.mkdirSync(path.join(root, "Components"));
  fs.writeFileSync(path.join(root, "settings.cfg"), SETTINGS);
  return root;
}

function resourcesWithBunnySplit() {
  const resources = tempDir();
  fs.mkdirSync(path.join(resources, LiveSplitSplits.COMPONENTS_DIRECTORY));
  fs.writeFileSync(path.join(resources, LiveSplitSplits.COMPONENTS_DIRECTORY, LiveSplitSplits.BUNNYSPLIT), "dll");
  return resources;
}

test("у каждой игры свои наборы, категории speedrun.com", () => {
  assert.deepEqual(LiveSplitSplits.describe("ghosting-renewed").map((p) => p.id), ["chapters", "maps"]);
  assert.deepEqual(LiveSplitSplits.describe("hl1-won").map((p) => p.category), ["Scripted", "Scriptless"]);
  for (const gameId of ["hl2-2707", "hl2-4260"])
    assert.deepEqual(
      [...new Set(LiveSplitSplits.describe(gameId).map((p) => p.category))],
      ["With Scripts", "Scriptless"]
    );
  assert.deepEqual(
    [...new Set(LiveSplitSplits.describe("hl2-5288").map((p) => p.category))],
    ["Any%", "Any% No Voidclip"]
  );
  assert.deepEqual(LiveSplitSplits.describe("unknown-game"), []);
  // Чужой id набора — первый набор игры.
  assert.equal(LiveSplitSplits.presetOf("hl1-won", "maps").id, "scripted");
});

test("Ghosting открывается со сплитами из архива LiveSplit", () => {
  const root = liveSplitRoot();
  const splits = new LiveSplitSplits({ resourcesPath: tempDir() });
  const prepared = splits.prepare(root, "ghosting-renewed", "maps");
  assert.deepEqual(prepared.args, ["-s", "splits\\maps\\maps.lss", "-l", "splits\\maps\\maps.lsl"]);
  assert.equal(fs.readFileSync(path.join(root, "splits/maps/maps.lss"), "utf8"), "<Run />");
});

test("HL2: сплиты с SourceSplit пишутся один раз и попадают в settings.cfg", () => {
  const root = liveSplitRoot();
  const splits = new LiveSplitSplits({ resourcesPath: tempDir() });
  const prepared = splits.prepare(root, "hl2-4260", "scriptless-chapters");
  assert.deepEqual(prepared.args, [
    "-s", "splits\\hlsr\\hl2-4260\\scriptless-chapters.lss",
    "-l", "splits\\hlsr\\hl2-4260\\scriptless-chapters.lsl",
  ]);
  const file = path.join(root, "splits/hlsr/hl2-4260/scriptless-chapters.lss");
  const text = fs.readFileSync(file, "utf8");
  assert.match(text, /<GameName>Half-Life 2<\/GameName>/);
  assert.match(text, /<Variable name="Scripts">Scriptless<\/Variable>/);
  assert.match(text, /<Variable name="Build">4231-4260<\/Variable>/);
  assert.equal((text.match(/<Segment>/g) || []).length, 14);
  assert.match(text, /<MTLModeSetting>Allow<\/MTLModeSetting>/);

  const settings = fs.readFileSync(path.join(root, "settings.cfg"), "utf8");
  assert.match(settings, /lastTimingMethod="GameTime"[^>]*>splits\\hlsr\\hl2-4260\\scriptless-chapters\.lss</);
  assert.match(settings, /<AutoSplitter>Half-Life 2<\/AutoSplitter>/);

  // Рекорды в файле не затираются при следующем запуске.
  fs.writeFileSync(file, "PB");
  splits.prepare(root, "hl2-4260", "scriptless-chapters");
  assert.equal(fs.readFileSync(file, "utf8"), "PB");
  assert.equal((fs.readFileSync(path.join(root, "settings.cfg"), "utf8").match(/scriptless-chapters\.lss/g) || []).length, 1);
});

test("HL1: раскладка с BunnySplit, компонент кладётся в Components", () => {
  const root = liveSplitRoot();
  const splits = new LiveSplitSplits({ resourcesPath: resourcesWithBunnySplit() });
  const prepared = splits.prepare(root, "hl1-won", "scriptless");
  assert.equal(prepared.args[1], "splits\\hlsr\\hl1-won\\scriptless.lss");
  const layout = fs.readFileSync(path.join(root, "splits/hlsr/hl1-won/scriptless.lsl"), "utf8");
  assert.match(layout, /<Path>LiveSplit\.BunnySplit\.dll<\/Path>/);
  assert.match(layout, /<SplitOnHL1Chapters>True<\/SplitOnHL1Chapters>/);
  assert.ok(layout.indexOf("BunnySplit") < layout.indexOf("</Components>"));
  assert.ok(fs.existsSync(path.join(root, "Components", LiveSplitSplits.BUNNYSPLIT)));
  const run = fs.readFileSync(path.join(root, "splits/hlsr/hl1-won/scriptless.lss"), "utf8");
  assert.match(run, /<Variable name="Scripts">Scriptless<\/Variable>/);
  assert.equal((run.match(/<Segment>/g) || []).length, 17);
  // У HL1 нет SourceSplit: список автосплиттеров не трогается.
  assert.doesNotMatch(fs.readFileSync(path.join(root, "settings.cfg"), "utf8"), /<AutoSplitter>/);
});

const sha256 = (data) => require("crypto").createHash("sha256").update(data).digest("hex");

test("BunnySplit: поставляемый файл закреплён", () => {
  const shipped = fs.readFileSync(path.join(ROOT, "resources", LiveSplitSplits.COMPONENTS_DIRECTORY, LiveSplitSplits.BUNNYSPLIT));
  assert.equal(sha256(shipped), LiveSplitSplits.BUNNYSPLIT_SHA256);
  assert.ok(!LiveSplitSplits.BUNNYSPLIT_BROKEN_SHA256.includes(LiveSplitSplits.BUNNYSPLIT_SHA256));
  assert.deepEqual(LiveSplitSplits.BUNNYSPLIT_BROKEN_SHA256, ["0c6a8d199f8ef6c5e3bc32239569f017568220b03314a5424a56acd714279794"]);
});

test("BunnySplit: сломанная копия заменяется", () => {
  const root = liveSplitRoot();
  const target = path.join(root, "Components", LiveSplitSplits.BUNNYSPLIT);
  fs.writeFileSync(target, "broken");
  const messages = [];
  const splits = new LiveSplitSplits({
    resourcesPath: resourcesWithBunnySplit(),
    brokenSha256: [sha256("broken")],
    logger: { info: (m) => messages.push(m), warn() {} },
  });
  assert.equal(splits.installBunnySplit(root), true);
  assert.equal(fs.readFileSync(target, "utf8"), "dll");
  assert.ok(messages.some((m) => m.includes("заменён сломанный BunnySplit")));
  assert.deepEqual(fs.readdirSync(path.join(root, "Components")), [LiveSplitSplits.BUNNYSPLIT]);
});

test("BunnySplit: при ошибке переименования .tmp не остаётся", () => {
  const root = liveSplitRoot();
  const failing = { ...fs, renameSync() { throw new Error("EPERM"); } };
  const splits = new LiveSplitSplits({ fs: failing, resourcesPath: resourcesWithBunnySplit() });
  assert.throws(() => splits.installBunnySplit(root), /EPERM/);
  assert.deepEqual(fs.readdirSync(path.join(root, "Components")), []);
});

test("BunnySplit: своя версия пользователя не трогается", () => {
  const root = liveSplitRoot();
  const target = path.join(root, "Components", LiveSplitSplits.BUNNYSPLIT);
  fs.writeFileSync(target, "mine");
  const splits = new LiveSplitSplits({ resourcesPath: resourcesWithBunnySplit(), brokenSha256: [sha256("broken")] });
  assert.equal(splits.installBunnySplit(root), false);
  assert.equal(fs.readFileSync(target, "utf8"), "mine");
});

function fakeChild() {
  const child = new EventEmitter();
  child.pid = 4242;
  child.exitCode = null;
  child.unref = () => {};
  return child;
}

function toolService(root, spawned) {
  const service = new ToolService({
    platform: "win32",
    paths: { root: path.dirname(root), toolsDir: path.dirname(root) },
    resourcesPath: resourcesWithBunnySplit(),
    spawnFn: (exe, args) => {
      const child = fakeChild();
      spawned.push({ exe, args, child });
      return child;
    },
    stopper: {
      graceful: async () => {
        const last = spawned[spawned.length - 1];
        last.child.exitCode = 0;
        last.child.emit("exit", 0);
        return { ok: true };
      },
    },
  });
  service.readInstalled = () => ({ executable: "LiveSplit.exe" });
  service.manifestOf = () => ({ executable: "LiveSplit.exe" });
  service.installPathOf = () => root;
  return service;
}

test("ToolService: LiveSplit стартует со сплитами игры, другие — через перезапуск", async () => {
  const root = liveSplitRoot();
  fs.writeFileSync(path.join(root, "LiveSplit.exe"), "");
  const spawned = [];
  const service = toolService(root, spawned);

  await service.launch("livesplit", { gameId: "hl2-2707", splits: "scripts-maps" });
  assert.deepEqual(spawned[0].args.slice(0, 2), ["-s", "splits\\hlsr\\hl2-2707\\scripts-maps.lss"]);
  assert.deepEqual(service.runningInfo("livesplit"), { gameId: "hl2-2707", splits: "scripts-maps" });

  const same = await service.launch("livesplit", { gameId: "hl2-2707", splits: "scripts-maps" });
  assert.equal(same.status, "already-running");
  assert.equal(same.same, true);

  const other = await service.launch("livesplit", { gameId: "hl1-won", splits: "scripted" });
  assert.equal(other.status, "already-running");
  assert.equal(other.same, false);
  assert.equal(spawned.length, 1);

  await service.launch("livesplit", { gameId: "hl1-won", splits: "scripted", restart: true });
  assert.equal(spawned.length, 2);
  assert.equal(spawned[1].args[1], "splits\\hlsr\\hl1-won\\scripted.lss");
  assert.deepEqual(service.runningInfo("livesplit"), { gameId: "hl1-won", splits: "scripted" });
});

test("оверлей LiveSplit выключен в играх, для которых LiveSplit не открыт", () => {
  const base = tempDir();
  for (const gameId of ["hl1-won", "hl2-4260"]) fs.mkdirSync(path.join(base, "livesplit-overlay", gameId), { recursive: true });
  const overlay = new LiveSplitOverlay({ baseDir: base });
  const settingsOf = () => ({ liveSplitOverlay: { enabled: true } });
  const ini = (gameId) => fs.readFileSync(path.join(base, "livesplit-overlay", gameId, "overlay.ini"), "utf8");

  assert.equal(overlay.bindTo("hl1-won", settingsOf), true);
  assert.match(ini("hl1-won"), /^enabled=1$/m);
  assert.match(ini("hl2-4260"), /^enabled=0$/m);

  assert.equal(overlay.bindTo("hl1-won", settingsOf), false);

  // LiveSplit закрыли — оверлей снова во всех играх.
  overlay.bindTo(null, settingsOf);
  assert.match(ini("hl2-4260"), /^enabled=1$/m);
});

test("время из LiveSplit в Ghosting: перед запуском включается TCP-сервер на порту из настроек", () => {
  const fsMod = require("fs");
  const osMod = require("os");
  const pathMod = require("path");
  const Splits = require("../src/main/game-delivery/livesplit-splits");
  const root = fsMod.mkdtempSync(pathMod.join(osMod.tmpdir(), "hlsr-ls-server-"));
  const file = pathMod.join(root, "settings.cfg");
  const base = "<Settings>\r\n  <ServerPort>16834</ServerPort>\r\n  <ServerStartup>0</ServerStartup>\r\n  <ServerState>0</ServerState>\r\n</Settings>\r\n";
  fsMod.writeFileSync(file, base);
  const splits = new Splits({ resourcesPath: root });

  assert.equal(splits.enableServer(root, 17000), true);
  let text = fsMod.readFileSync(file, "utf8");
  assert.match(text, /<ServerStartup>1<\/ServerStartup>/);
  assert.match(text, /<ServerPort>17000<\/ServerPort>/);
  assert.equal(splits.enableServer(root, 17000), false, "повторно файл не трогается");

  // «Как в прошлый раз» с TCP оставляем, с WebSocket — переключаем на TCP.
  fsMod.writeFileSync(file, base.replace("<ServerStartup>0", "<ServerStartup>3").replace("<ServerState>0", "<ServerState>1"));
  splits.enableServer(root, 16834);
  assert.match(fsMod.readFileSync(file, "utf8"), /<ServerStartup>3<\/ServerStartup>/);
  fsMod.writeFileSync(file, base.replace("<ServerStartup>0", "<ServerStartup>2"));
  splits.enableServer(root, 16834);
  assert.match(fsMod.readFileSync(file, "utf8"), /<ServerStartup>1<\/ServerStartup>/);

  // Старый settings.cfg без тегов сервера.
  fsMod.writeFileSync(file, "<Settings>\r\n  <RefreshRate>40</RefreshRate>\r\n</Settings>\r\n");
  splits.enableServer(root, 16834);
  text = fsMod.readFileSync(file, "utf8");
  assert.match(text, /<ServerStartup>1<\/ServerStartup>/);
  assert.match(text, /<ServerPort>16834<\/ServerPort>\r\n<\/Settings>/);
});
