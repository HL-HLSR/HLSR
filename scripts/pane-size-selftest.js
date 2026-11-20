"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { STEP, clampSize, readSize, writeSize, keySize } = require("../src/renderer/components/Launcher/pane-size");

// localStorage в памяти; broken — хранилище, которое бросает (приватный режим, квота).
function memory(broken = false) {
  const map = new Map();
  const check = () => { if (broken) throw new Error("storage off"); };
  return {
    map,
    getItem: (key) => (check(), map.has(key) ? map.get(key) : null),
    setItem: (key, value) => (check(), map.set(key, String(value))),
    removeItem: (key) => (check(), map.delete(key)),
  };
}

test("разделитель панелей: размер зажимается в пределы, при тесном окне побеждает минимум", () => {
  assert.equal(clampSize(250.4, 130, 400), 250);
  assert.equal(clampSize(90, 130, 400), 130);
  assert.equal(clampSize(900, 130, 400), 400);
  assert.equal(clampSize(300, 130, 100), 130);
});

test("разделитель панелей: стрелки двигают на 16 px в пределах, Home и End — к краям, прочие клавиши не трогают", () => {
  assert.equal(STEP, 16);
  assert.equal(keySize(200, "ArrowDown", 130, 400), 216);
  assert.equal(keySize(200, "ArrowRight", 130, 400), 216);
  assert.equal(keySize(200, "ArrowUp", 130, 400), 184);
  assert.equal(keySize(200, "ArrowLeft", 130, 400), 184);
  assert.equal(keySize(395, "ArrowDown", 130, 400), 400);
  assert.equal(keySize(135, "ArrowUp", 130, 400), 130);
  assert.equal(keySize(200, "Home", 130, 400), 130);
  assert.equal(keySize(200, "End", 130, 400.6), 401);
  assert.equal(keySize(200, "Enter", 130, 400), null);
  assert.equal(keySize(200, "constructor", 130, 400), null);
});

test("разделитель панелей: размер запоминается по ключу, сброс забывает, мусор и сбой хранилища — размер по умолчанию", () => {
  const storage = memory();
  assert.equal(readSize(storage, "keyboard:hl1-won"), null);
  writeSize(storage, "keyboard:hl1-won", 311.6);
  assert.equal(storage.map.get("hlsr.pane.keyboard:hl1-won"), "312");
  assert.equal(readSize(storage, "keyboard:hl1-won"), 312);
  assert.equal(readSize(storage, "keyboard:ghosting-renewed"), null);
  writeSize(storage, "keyboard:hl1-won", null);
  assert.equal(readSize(storage, "keyboard:hl1-won"), null);

  for (const junk of ["", "abc", "-40", "0", "Infinity"]) {
    storage.map.set("hlsr.pane.x", junk);
    assert.equal(readSize(storage, "x"), null, junk);
  }

  const broken = memory(true);
  assert.doesNotThrow(() => writeSize(broken, "x", 200));
  assert.equal(readSize(broken, "x"), null);
  assert.equal(readSize(null, "x"), null);
  assert.doesNotThrow(() => writeSize(null, "x", 200));
});
