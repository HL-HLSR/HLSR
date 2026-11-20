"use strict";

// Замороженные копии модулей обновлялки (наборы 0.4.7 — tools/compat-047/src и 0.4.9 —
// tools/compat-047/src-049) — честный эталон приёмки. Тест сверяет их побайтно с эталоном набора
// (~/HLSR-electron, ~/HLSR-electron-049; HLSR_ELECTRON, HLSR_ELECTRON_049), проверяет, что список
// frozen.json и файлы на диске совпадают, и что каждая копия грузится «как на другой ОС»
// (load-as.js). Запуск:
//   node --test tools/compat-047/tests/*.test.js
// Без эталонной копии (CI, чужая машина) сверка побайтно пропускается с причиной, остальное идёт.

const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs");
const path = require("path");
const freeze = require("../freeze");
const { loadAs } = require("../lib/load-as");
const { parseArgs } = require("../accept-release");

test("наборы 0.4.7 и 0.4.9; база 0.4.8 проверяется набором 0.4.7", () => {
  assert.deepEqual(freeze.SETS, ["0.4.7", "0.4.9"]);
  assert.deepEqual(freeze.BASES, { "0.4.7": "0.4.7", "0.4.8": "0.4.7", "0.4.9": "0.4.9" });
  assert.throws(() => freeze.setForBase("0.4.6"), /не поддерживается/);
});

test("список frozen.json и файлы каждого набора совпадают, лишнего нет", () => {
  assert.ok(freeze.FILES.length >= 28);
  assert.equal(new Set(freeze.FILES).size, freeze.FILES.length, "повторы в списке");
  for (const set of freeze.SETS) {
    assert.deepEqual(freeze.check(set, { compareWithReference: false }), []);
    for (const file of freeze.FILES) assert.ok(fs.statSync(path.join(freeze.setDir(set), ...file.split("/"))).isFile(), `${set}: ${file}`);
  }
});

for (const set of freeze.SETS) {
  test(`копии ${set} побайтно равны модулям эталона (cmp)`, (t) => {
    const missing = freeze.referenceMissing(set);
    if (missing) return t.skip(`${missing}: сверка побайтно пропущена`);
    assert.deepEqual(freeze.check(set), []);
    for (const file of freeze.FILES) {
      const copy = fs.readFileSync(path.join(freeze.setDir(set), ...file.split("/")));
      const reference = fs.readFileSync(path.join(freeze.referenceRoot(set), "src", ...file.split("/")));
      assert.equal(Buffer.compare(copy, reference), 0, file);
    }
  });
}

test("набор 0.4.9 отличается от 0.4.7 только файлами, которые 0.4.9 поменяла", () => {
  // git diff 62d0647 v0.4.9 в области frozen.json: остальное у 0.4.9 то же, что у 0.4.7/0.4.8.
  const changed = freeze.FILES.filter((file) => Buffer.compare(
    fs.readFileSync(path.join(freeze.setDir("0.4.7"), ...file.split("/"))),
    fs.readFileSync(path.join(freeze.setDir("0.4.9"), ...file.split("/")))
  ) !== 0);
  assert.deepEqual(changed, [
    "main/game-delivery/errors.js",
    "main/launcher-update/LauncherUpdateGate.js",
    "main/windows-install/layout.js",
    "shared/delivery-endpoints.js",
    "updater/core/install.js",
  ]);
});

test("копии не содержат переводов строк CRLF (сравнение побайтно не зависит от autocrlf)", () => {
  // .gitattributes: tools/compat-047/** -text. Эталон с LF; копия с CRLF не равна ему.
  for (const set of freeze.SETS) {
    for (const file of freeze.FILES) {
      const bytes = fs.readFileSync(path.join(freeze.setDir(set), ...file.split("/")));
      assert.equal(bytes.includes(Buffer.from("\r\n")), false, `${set}: ${file}`);
    }
  }
});

test("каждая копия грузится для win32 и для linux, пути берутся по подменённой ОС", () => {
  // Модули платформы ведут себя по подмене: win32 → path.win32, linux → path.posix.
  const layout = loadAs("main/windows-install/layout.js", "win32", "0.4.7");
  const plan = loadAs("updater/core/plan.js", "win32", "0.4.7");
  assert.equal(typeof layout.identity, "function");
  assert.equal(typeof plan.validate, "function");
  // Набор 0.4.9 грузится из своего каталога: identityOfGuid есть только в layout.js 0.4.9.
  assert.equal(layout.identityOfGuid, undefined);
  assert.equal(typeof loadAs("main/windows-install/layout.js", "win32", "0.4.9").identityOfGuid, "function");
  for (const set of freeze.SETS) {
    for (const platform of ["win32", "linux"]) {
      for (const file of freeze.FILES) {
        // Модули updater/shared/* — копии тех же исходников для упакованного апдейтера: грузятся так же.
        assert.doesNotThrow(() => loadAs(file, platform, set), `${set}: ${file} (${platform})`);
      }
    }
  }
  assert.throws(() => loadAs("main/nothing.js", "win32", "0.4.7"), /нет замороженного модуля/);
  // Без набора — отказ, а не молчаливый 0.4.7.
  assert.throws(() => loadAs("main/windows-install/layout.js", "win32"), /нет набора/);
});

test("accept-release: базы по умолчанию 0.4.8 и 0.4.9, --base выбирает, чужая база — ошибка", () => {
  assert.deepEqual(parseArgs(["--dist", "x"]).bases, ["0.4.8", "0.4.9"]);
  assert.deepEqual(parseArgs(["--dist", "x", "--base", "0.4.9"]).bases, ["0.4.9"]);
  assert.deepEqual(parseArgs(["--dist", "x", "--base", "0.4.8, 0.4.9"]).bases, ["0.4.8", "0.4.9"]);
  assert.throws(() => parseArgs(["--dist", "x", "--base", "0.4.6"]), /не поддерживается/);
  assert.throws(() => parseArgs(["--dist", "x", "--base", ","]), /хотя бы одна/);
});
