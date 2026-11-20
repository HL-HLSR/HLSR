"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const doc = require("../src/shared/cfg-document");

const edit = (text, fn) => doc.serialize(fn(doc.parse(text)));

test("set в середине меняет только значение", () => {
  assert.equal(edit("a 1\r\nfov 90 // мой\r\nb 2\r\n", (d) => doc.set(d, "cvar:fov", "110", "settings")),
    "a 1\r\nfov 110 // мой\r\nb 2\r\n");
});

test("set команды после «;» не трогает соседей", () => {
  assert.equal(edit("crosshair 1; default_fov 90\n", (d) => doc.set(d, "cvar:default_fov", "100", "settings")),
    "crosshair 1; default_fov 100\n");
});

test("внутри alias ключ не ищется", () => {
  const text = 'alias x "default_fov 120"\ndefault_fov 90\n';
  assert.equal(edit(text, (d) => doc.set(d, "cvar:default_fov", "100", "settings")),
    'alias x "default_fov 120"\ndefault_fov 100\n');
  assert.equal(doc.get(doc.parse('alias x "default_fov 120"\n'), "cvar:default_fov"), null);
});

test("set бинда выше unbindall: строка не правится на месте, новый бинд в раздел после unbindall", () => {
  const text = `${doc.SECTIONS.settings}\nbind p "run"\n\nunbindall\nbind w "+forward"\n`;
  assert.equal(edit(text, (d) => doc.set(d, "bind:p", "run", "binds")),
    `${text}\n${doc.SECTIONS.binds}\nbind "p" "run"\n`);
  // После unbindall бинд правится на месте, как раньше.
  assert.equal(edit("unbindall\nbind p x\n", (d) => doc.set(d, "bind:p", "y", "binds")), "unbindall\nbind p y\n");
});

test("N3: три set бинда при разделе binds выше unbindall — одна строка после него, в игре последнее значение", () => {
  const { runChain } = require("./lib/cfg-state");
  let d = doc.parse(`${doc.SECTIONS.binds}\nbind "f" "+use"\n\n${doc.SECTIONS.own}\nunbindall\nbind w "+forward"\n`);
  for (const v of ["+jump", "+duck", "+attack"]) d = doc.set(d, "bind:f", v, "binds");
  const text = doc.serialize(d);
  const after = text.slice(text.indexOf("unbindall"));
  assert.equal((after.match(/bind "f"/g) || []).length, 1, text);
  assert.equal(runChain((n) => (n === "config.cfg" ? text : null), ["config.cfg"]).binds.get("f"), "+attack");
});
test("из повторов меняется последняя", () => {
  assert.equal(edit("fov 1\nfov 2\n", (d) => doc.set(d, "cvar:fov", "3", "settings")), "fov 1\nfov 3\n");
  assert.deepEqual(doc.get(doc.parse("fov 1\nfov 2\n"), "cvar:fov"), { value: "2", line: 1, index: 0 });
});

test("нет ключа, есть раздел: строка перед следующим заголовком", () => {
  const text = `${doc.SECTIONS.settings}\na 1\n\n${doc.SECTIONS.binds}\nbind x y\n`;
  assert.equal(edit(text, (d) => doc.set(d, "cvar:fov", "90", "settings")),
    `${doc.SECTIONS.settings}\na 1\nfov "90"\n\n${doc.SECTIONS.binds}\nbind x y\n`);
});

test("нет раздела, есть own: заголовок и строка перед ним", () => {
  const text = `a 1\n${doc.SECTIONS.own}\nb 2\n`;
  assert.equal(edit(text, (d) => doc.set(d, "cvar:fov", "90", "settings")),
    `a 1\n${doc.SECTIONS.settings}\nfov "90"\n\n${doc.SECTIONS.own}\nb 2\n`);
});

test("нет ничего: в конец с переводом строки файла, без перевода в конце — сначала он", () => {
  assert.equal(edit("a 1\r\nb 2", (d) => doc.set(d, "cvar:fov", "90", "settings")),
    `a 1\r\nb 2\r\n\r\n${doc.SECTIONS.settings}\r\nfov "90"\r\n`);
  assert.equal(edit("", (d) => doc.set(d, "cvar:fov", "90", "settings")), `${doc.SECTIONS.settings}\r\nfov "90"\r\n`);
});

test("bind и unbind: ключ и замена команды", () => {
  assert.equal(edit('bind "F" "+use"\n', (d) => doc.set(d, "bind:f", "+jump", "binds")), 'bind "F" "+jump"\n');
  assert.equal(doc.keyOf([{ value: "unbind" }, { value: "F" }]), "bind:f");
});

test("keyOf: глаголы без значения дают null", () => {
  const args = (...v) => v.map((value) => ({ value }));
  assert.equal(doc.keyOf(args("exec", "a.cfg")), null);
  assert.equal(doc.keyOf(args("echo", "hi")), null);
  assert.equal(doc.keyOf(args("+attack")), null);
  assert.equal(doc.keyOf(args("FOV", "90")), "cvar:fov");
  assert.equal(doc.keyOf(args("alias", "Run", "x")), "alias:run");
});

test("removeKey: команда в строке с «;» и целая строка", () => {
  assert.equal(edit("bind f x; echo hi\nbind f y\n", (d) => doc.removeKey(d, "bind:f")), "echo hi\n");
  assert.equal(edit("a 1; bind f x; b 2\n", (d) => doc.removeKey(d, "bind:f")), "a 1; b 2\n");
  assert.equal(edit("a 1; bind f x\n", (d) => doc.removeKey(d, "bind:f")), "a 1\n");
});

test("блоки: создать, заменить, удалить", () => {
  let d = doc.setBlock(doc.parse("a 1\n"), "run", ['alias run "x"'], "scripts");
  assert.equal(doc.serialize(d), `a 1\n\n${doc.SECTIONS.scripts}\n// HLSR script run begin\nalias run "x"\n// HLSR script run end\n`);
  d = doc.setBlock(d, "run", ['alias run "y"', "echo z"], "scripts");
  assert.deepEqual(doc.findBlock(d, "run").body, ['alias run "y"', "echo z"]);
  assert.equal(doc.serialize(doc.removeBlock(d, "run")), `a 1\n\n${doc.SECTIONS.scripts}\n`);
  assert.equal(doc.findBlock(doc.parse("a 1\n"), "run"), null);
});

test("latin1 round-trip: остальные байты не меняются", () => {
  const high = Buffer.from(Array.from({ length: 128 }, (_, i) => 0x80 + i));
  const source = Buffer.concat([Buffer.from("// "), high, Buffer.from("\r\nfov 90\r\nb 2\r\n")]);
  const out = Buffer.from(edit(source.toString("latin1"), (d) => doc.set(d, "cvar:fov", "110", "settings")), "latin1");
  assert.deepEqual(out, Buffer.concat([Buffer.from("// "), high, Buffer.from("\r\nfov 110\r\nb 2\r\n")]));
});

test("BOM сохраняется", () => {
  assert.ok(edit("﻿fov 1\n", (d) => doc.set(d, "cvar:fov", "2", "settings")).startsWith("﻿fov 2"));
});

test("unbind после bind: get даёт null, set заменяет unbind на месте", () => {
  const text = "bind f x\nunbind f\n";
  assert.equal(doc.get(doc.parse(text), "bind:f"), null);
  assert.equal(edit(text, (d) => doc.set(d, "bind:f", "y", "binds")), 'bind f x\nbind "f" "y"\n');
  assert.equal(edit("a 1; unbind F // к\n", (d) => doc.set(d, "bind:f", "y", "binds")), 'a 1; bind "f" "y" // к\n');
  assert.deepEqual(doc.get(doc.parse("unbind f\nbind f z\n"), "bind:f"), { value: "z", line: 1, index: 0 });
});

test("одиночный \\r: текст не теряется", () => {
  const text = "a 1\rfov 90\nb 2\r";
  assert.equal(doc.serialize(doc.parse(text)), text);
  assert.equal(edit(text, (d) => doc.set(d, "cvar:fov", "100", "settings")), "a 1\rfov 100\nb 2\r");
});
