"use strict";

// Поиск, замена и операции со строками редактора cfg (src/shared/cfg-text-ops.js).

const test = require("node:test");
const assert = require("node:assert/strict");
const ops = require("../src/shared/cfg-text-ops");

function apply(text, edit) {
  return text.slice(0, edit.start) + edit.insert + text.slice(edit.end);
}

test("поиск: регистр, слово целиком, регулярка, ошибка выражения", () => {
  const text = "bind w +forward\nBIND s +back\nunbind w";
  assert.equal(ops.findAll(text, "bind").matches.length, 3);
  assert.equal(ops.findAll(text, "bind", { caseSensitive: true }).matches.length, 2);
  assert.equal(ops.findAll(text, "bind", { wholeWord: true }).matches.length, 2);
  assert.deepEqual(ops.findAll(text, "\\+\\w+", { regex: true }).matches.map((m) => text.slice(m.start, m.end)), ["+forward", "+back"]);
  assert.equal(ops.findAll(text, "a.b").matches.length, 0, "без режима регулярки точка — точка");
  assert.equal(ops.findAll(text, "(", { regex: true }).error, "BAD_REGEX");
  assert.equal(ops.findAll(text, "x*", { regex: true }).matches.length, 0, "пустые совпадения пропускаются");
  assert.equal(ops.findAll(text, "").error, null);
});

test("ближайшее совпадение вперёд и назад, по кругу", () => {
  const matches = [{ start: 2, end: 4 }, { start: 10, end: 12 }];
  assert.equal(ops.nearestMatch(matches, 0), 0);
  assert.equal(ops.nearestMatch(matches, 5), 1);
  assert.equal(ops.nearestMatch(matches, 13), 0);
  assert.equal(ops.nearestMatch(matches, 10, true), 0);
  assert.equal(ops.nearestMatch(matches, 1, true), 1);
});

test("замена: одна и все, группы в регулярке", () => {
  const text = "sensitivity 2\nm_yaw 0.022\nsensitivity 3";
  const all = ops.replaceAll(text, "sensitivity (\\d)", "sensitivity $1.5", { regex: true });
  assert.equal(all.count, 2);
  assert.equal(apply(text, all.edit), "sensitivity 2.5\nm_yaw 0.022\nsensitivity 3.5");
  assert.equal(apply(text, ops.replaceAll(text, "$1", "x").edit || { start: 0, end: 0, insert: "" }), text);
  const { matches } = ops.findAll(text, "sensitivity");
  const one = ops.replaceOne(text, matches[1], "sensitivity", "sens", {});
  assert.equal(apply(text, one), "sensitivity 2\nm_yaw 0.022\nsens 3");
  assert.equal(ops.replaceAll(text, "$", "€").count, 0, "доллар без регулярки ищется буквально");
});

test("комментарий: ставится по наименьшему отступу и снимается", () => {
  const text = "a\n  b\n\nc";
  const on = ops.toggleComment(text, 0, text.length);
  assert.equal(apply(text, on), "// a\n//   b\n\n// c");
  const commented = apply(text, on);
  assert.equal(apply(commented, ops.toggleComment(commented, 0, commented.length)), text);
  const indented = "  x\n    y";
  assert.equal(apply(indented, ops.toggleComment(indented, 0, 1)), "  // x\n    y");
});

test("строки: дублирование, удаление, перенос, отступ", () => {
  const text = "one\ntwo\nthree";
  assert.equal(apply(text, ops.duplicateLines(text, 5, 5)), "one\ntwo\ntwo\nthree");
  assert.equal(apply(text, ops.deleteLines(text, 5, 5)), "one\nthree");
  assert.equal(apply(text, ops.deleteLines(text, 10, 10)), "one\ntwo");
  assert.equal(apply(text, ops.moveLines(text, 5, 5, -1)), "two\none\nthree");
  assert.equal(apply(text, ops.moveLines(text, 5, 5, 1)), "one\nthree\ntwo");
  assert.equal(ops.moveLines(text, 0, 0, -1), null);
  assert.equal(ops.moveLines(text, 10, 10, 1), null);
  // выделение до начала следующей строки её не захватывает
  assert.equal(apply(text, ops.indentLines(text, 0, 8)), "\tone\n\ttwo\nthree");
  assert.equal(apply("\tone\n  two", ops.indentLines("\tone\n  two", 0, 9, true)), "one\ntwo");
});

test("строка и столбец, начало строки", () => {
  const text = "ab\ncde\nf";
  assert.deepEqual(ops.lineCol(text, 4), { line: 2, col: 2 });
  assert.equal(ops.lineStart(text, 3), 7);
  assert.equal(ops.lineStart(text, 9), text.length);
  assert.equal(ops.lineCount(text), 3);
});
