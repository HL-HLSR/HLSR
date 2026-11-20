"use strict";

// Экспорт и импорт сборки: в файл попадают только cfg игрока, импорт
// пишет их только внутрь папки игры.

const test = require("node:test");
const assert = require("node:assert");
const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const buildTransfer = require("../src/main/game-delivery/build-transfer");
const GameSettingsStore = require("../src/main/game-delivery/GameSettingsStore");

const sha = (text) => crypto.createHash("sha256").update(text).digest("hex");

function settingsStore(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-build-settings-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return new GameSettingsStore({ file: path.join(root, "game-settings.json") });
}

test("настройки из файла не задают команды Proton: остаются свои", (t) => {
  const store = settingsStore(t);
  store.save("ghosting-renewed", { protonOptions: "mangohud %command%", console: true });
  const saved = store.saveImported("ghosting-renewed", { protonOptions: 'sh -c "id > /tmp/pwned" %command%', console: false, novid: true });
  assert.equal(saved.ok, true);
  assert.equal(saved.settings.protonOptions, "mangohud %command%");
  assert.equal(saved.settings.console, false);
  assert.equal(saved.settings.novid, true);
});

test("настройки из файла не задают команды и пути в параметрах запуска: остаются свои", (t) => {
  const store = settingsStore(t);
  store.save("ghosting-renewed", { launchArgs: "-novid" });
  for (const args of ["+exec //host/share/a.cfg", "+connect 203.0.113.1", "-config //host/a", "-config C:/a.cfg", "-x ../a"]) {
    const saved = store.saveImported("ghosting-renewed", { launchArgs: args });
    assert.equal(saved.ok, true, args);
    assert.equal(saved.settings.launchArgs, "-novid", args);
  }
});

test("обычные параметры запуска из файла переносятся", (t) => {
  const store = settingsStore(t);
  const saved = store.saveImported("ghosting-renewed", { launchArgs: "-w 1920 -h 1080 -freq 144 -high" });
  assert.equal(saved.ok, true);
  assert.equal(saved.settings.launchArgs, "-w 1920 -h 1080 -freq 144 -high");
});

function makeGame() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-build-transfer-"));
  const write = (rel, data) => {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.writeFileSync(path.join(root, rel), data);
  };
  write("valve/config.cfg", "stock\n");
  write("valve/autoexec.cfg", "мой autoexec\n");
  write("valve/userconfig.cfg", "stock user\n");
  write("valve/scripts/jump.cfg", "alias j +jump\n");
  const previous = {
    files: [
      { path: "valve/config.cfg", sha256: sha("stock\n") },
      { path: "valve/autoexec.cfg", sha256: sha("stock autoexec\n") },
      { path: "valve/userconfig.cfg", sha256: sha("stock user\n") },
    ],
  };
  return { root, previous };
}

test("build-transfer: экспорт — только изменённые и свои cfg, импорт возвращает их", () => {
  const { root, previous } = makeGame();
  const files = buildTransfer.collect(root, previous);
  assert.deepStrictEqual(files.map((f) => f.path).sort(), ["valve/autoexec.cfg", "valve/scripts/jump.cfg"]);

  const text = buildTransfer.serialize({ launcherId: "hl1", gameId: "hl1-won", version: "1.0.0", settings: { fullscreen: true }, files });
  const parsed = buildTransfer.parse(text, { isKnownGame: (id) => id === "hl1-won" });
  assert.equal(parsed.gameId, "hl1-won");
  assert.deepStrictEqual(parsed.settings, { fullscreen: true });

  const target = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-build-target-"));
  fs.mkdirSync(path.join(target, "valve"));
  buildTransfer.applyFiles(target, parsed.files);
  assert.equal(fs.readFileSync(path.join(target, "valve/autoexec.cfg"), "utf8"), "мой autoexec\n");
  assert.equal(fs.readFileSync(path.join(target, "valve/scripts/jump.cfg"), "utf8"), "alias j +jump\n");
});

test("build-transfer: без files.json (сборка из Steam) переносятся все cfg", () => {
  const { root } = makeGame();
  assert.equal(buildTransfer.collect(root, null).length, 4);
});

test("build-transfer: занятый cfg — экспорт называет файл, окно советует закрыть программу", async (t) => {
  const GameDelivery = require("../src/main/game-delivery");
  const { root, previous } = makeGame();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const readFileSync = fs.readFileSync;
  t.mock.method(fs, "readFileSync", (file, ...rest) => {
    if (String(file).endsWith("autoexec.cfg")) throw Object.assign(new Error("EBUSY: resource busy or locked"), { code: "EBUSY" });
    return readFileSync(file, ...rest);
  });

  assert.throws(() => buildTransfer.collect(root, previous), { code: "FILE_LOCKED", file: "valve/autoexec.cfg" });
  const context = {
    resolveGameId: (id) => id,
    service: { installService: { readInstalled: () => ({ installPath: root, version: null }) } },
  };
  assert.deepStrictEqual(await GameDelivery.prototype.exportSettings.call(context, "hl1-won"), {
    status: "error",
    code: "FILE_LOCKED",
    file: "valve/autoexec.cfg",
  });

  for (const [lang, advice] of [["ru", "Закрой программу"], ["en", "Close the program"]]) {
    const locale = readFileSync(path.join(__dirname, `../src/renderer/utils/locales/${lang}.locale.js`), "utf8");
    assert.match(locale, new RegExp(`"#L_BT_ERR_FILE_LOCKED": "[^"]*%0%[^"]*${advice}`), lang);
  }
});

test("build-transfer: чужой формат, неизвестная игра, порча и выход из папки отклоняются", () => {
  const ok = (files) => JSON.stringify({ format: "hlsr-build", version: 1, game: { gameId: "hl1-won" }, files });
  const known = { isKnownGame: (id) => id === "hl1-won" };
  assert.throws(() => buildTransfer.parse("{}", known), { code: "BAD_FORMAT" });
  assert.throws(() => buildTransfer.parse("not json", known), { code: "BAD_FORMAT" });
  assert.throws(() => buildTransfer.parse(JSON.stringify({ format: "hlsr-build", version: 99, game: { gameId: "hl1-won" }, files: [] }), known), { code: "NEWER_FORMAT" });
  assert.throws(() => buildTransfer.parse(JSON.stringify({ format: "hlsr-build", version: 1, game: { gameId: "evil" }, files: [] }), known), { code: "UNKNOWN_GAME" });
  const data = Buffer.from("x").toString("base64");
  for (const bad of ["../x.cfg", "valve/../../x.cfg", "/etc/x.cfg", "C:/x.cfg", "valve\\x.cfg", "valve/x.exe", "valve//x.cfg"])
    assert.throws(() => buildTransfer.parse(ok([{ path: bad, data }]), known), { code: "BAD_FORMAT" }, bad);
  assert.throws(() => buildTransfer.parse(ok([{ path: "valve/a.cfg", data, sha256: "0".repeat(64) }]), known), { code: "CORRUPT" });
});

test("build-transfer: импорт не пишет через симлинк наружу", () => {
  const target = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-build-target-"));
  const outside = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-build-outside-"));
  try {
    fs.symlinkSync(outside, path.join(target, "valve"), "dir");
  } catch (error) {
    return;
  }
  assert.throws(() => buildTransfer.applyFiles(target, [{ path: "valve/autoexec.cfg", data: Buffer.from("x") }]), { code: "PATH_INVALID" });
  assert.equal(fs.readdirSync(outside).length, 0);
});
