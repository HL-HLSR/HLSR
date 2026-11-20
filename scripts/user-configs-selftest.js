"use strict";

// cfg игрока: патч, переустановка и проверка файлов не возвращают их к
// версии сборки (src/main/game-delivery/user-configs.js).

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");

const userConfigs = require("../src/main/game-delivery/user-configs");
const PatchService = require("../src/main/game-delivery/PatchService");
const IntegrityService = require("../src/main/game-delivery/IntegrityService");

const sha = (text) => crypto.createHash("sha256").update(text).digest("hex");

function fixture(t, files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-user-configs-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const [relative, text] of Object.entries(files)) {
    const full = path.join(root, relative);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, text);
  }
  return root;
}

const entry = (relative, text) => ({ path: relative, size: Buffer.byteLength(text), sha256: sha(text) });

test("патч не трогает изменённые игроком cfg, а стоковые обновляет", (t) => {
  const root = fixture(t, {
    "mod/stock.cfg": "старый\n",
    "mod/edited.cfg": "мой бинд\n",
    "mod/own.cfg": "мой файл\n",
    "mod/gone.cfg": "удалить\n",
    "mod/gone-edited.cfg": "удалить, но я менял\n",
    "mod/same.cfg": "новый\n",
    "mod/client.dll": "чужой бинарь",
  });
  const previous = { files: [
    entry("mod/stock.cfg", "старый\n"),
    entry("mod/edited.cfg", "стоковый\n"),
    entry("mod/gone.cfg", "удалить\n"),
    entry("mod/gone-edited.cfg", "удалить\n"),
    entry("mod/same.cfg", "старый\n"),
    entry("mod/client.dll", "бинарь"),
  ] };
  const patch = {
    fromVersion: "1.0.0",
    files: [
      entry("mod/stock.cfg", "новый\n"),
      entry("mod/edited.cfg", "новый\n"),
      entry("mod/own.cfg", "из сборки\n"),
      entry("mod/same.cfg", "новый\n"),
      entry("mod/missing.cfg", "новый\n"),
      entry("mod/client.dll", "новый бинарь"),
    ],
    deletedFiles: ["mod/gone.cfg", "mod/gone-edited.cfg"],
  };

  const kept = userConfigs.keptByPatch(root, patch, previous);
  assert.deepEqual(Array.from(kept).sort(), ["mod/edited.cfg", "mod/gone-edited.cfg", "mod/own.cfg"]);

  const service = new PatchService({ paths: {}, filesManifestOf: (gameId, version) => {
    assert.deepEqual([gameId, version], ["hl1-won", "1.0.0"]);
    return previous;
  } });
  const applied = service.withoutUserConfigs("hl1-won", root, patch);
  assert.deepEqual(applied.files.map((file) => file.path),
    ["mod/stock.cfg", "mod/same.cfg", "mod/missing.cfg", "mod/client.dll"]);
  assert.deepEqual(applied.deletedFiles, ["mod/gone.cfg"]);
  assert.equal(patch.files.length, 6, "исходный манифест патча не меняется");

  assert.equal(userConfigs.keptByPatch(root, patch, null).size, 0, "без прежней версии ничего не оставляется");
});

test("нечитаемый cfg считается изменённым и переносится, пропавший — нет", (t) => {
  const root = fixture(t, { "mod/locked.cfg": "стоковый\n" });
  const previous = { files: [entry("mod/locked.cfg", "стоковый\n"), entry("mod/missing.cfg", "стоковый\n")] };
  const lockedFs = Object.assign({}, fs, {
    readFileSync(file, ...rest) {
      if (String(file).endsWith("locked.cfg")) throw Object.assign(new Error("EBUSY: resource busy or locked"), { code: "EBUSY" });
      return fs.readFileSync(file, ...rest);
    },
  });
  assert.equal(userConfigs.isUserEdited(root, "mod/locked.cfg", previous), false, "читается и совпадает со сборкой");
  assert.equal(userConfigs.isUserEdited(root, "mod/locked.cfg", previous, { fs: lockedFs }), true, "не читается — не теряем");
  assert.equal(userConfigs.isUserEdited(root, "mod/missing.cfg", previous, { fs: lockedFs }), false, "нет файла — не изменён");
});

test("проверка файлов: изменённый cfg не повреждение, пропавший восстанавливается, оба видны отдельно", async (t) => {
  const root = fixture(t, {
    "mod/edited.cfg": "мой бинд\n",
    "mod/edited-same-size.cfg": "abc\n",
    "mod/client.dll": "битый",
  });
  const manifest = { gameId: "hl1-won", version: "1.0.0", files: [
    entry("mod/edited.cfg", "стоковый cfg\n"),
    entry("mod/edited-same-size.cfg", "xyz\n"),
    entry("mod/missing.cfg", "есть в сборке\n"),
    entry("mod/client.dll", "целый"),
  ] };
  const result = await new IntegrityService({}).check({ installPath: root, filesManifest: manifest });
  assert.deepEqual(result.missing, []);
  assert.deepEqual(result.configMissing, ["mod/missing.cfg"]);
  assert.deepEqual(result.configChanged.sort(), ["mod/edited-same-size.cfg", "mod/edited.cfg"]);
  assert.deepEqual(result.skipped, []);
  assert.deepEqual(result.corrupted.map((item) => item.path), ["mod/client.dll"]);
  assert.equal(result.damaged, 2);
});
