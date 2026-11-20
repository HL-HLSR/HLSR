"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readableName } = require("../src/shared/ansi-name");

// Как игра кладёт имя на диск: байты UTF-8, прочитанные Windows как cp1251.
const asWritten = (name) => new TextDecoder("windows-1251").decode(Buffer.from(name, "utf8"));

test("имя от игры (UTF-8 через cp1251) читается по-русски", () => {
  for (const name of ["спидран_005.run", "Пёс.run", "Ёжик.run", "призрак Гордона.run", "ран №3.dem"])
    assert.equal(readableName(asWritten(name)), name);
});

test("настоящие имена не меняются", () => {
  for (const name of ["myrun_005.run", "спидран_005.run", "Пёс.run", "Привет мир", "日本語.run", "", "a—b.run"])
    assert.equal(readableName(name), name);
  assert.equal(readableName(undefined), undefined);
});
