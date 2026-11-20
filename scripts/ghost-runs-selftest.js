"use strict";

const assert = require("node:assert/strict");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { test } = require("node:test");
const runs = require("../src/main/game-delivery/runs-scan");
const GameDelivery = require("../src/main/game-delivery");
const { NO_SYMLINKS } = require("./lib/selftest-caps");

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-ghost-delete-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const directory = path.join(root, "ghostingmod/runs");
  fs.mkdirSync(directory, { recursive: true });
  fs.writeFileSync(path.join(directory, "one.run"), "ghost one");
  fs.writeFileSync(path.join(directory, "two.run"), "ghost two");
  fs.writeFileSync(path.join(root, "outside.run"), "keep outside");
  fs.writeFileSync(path.join(directory, "config.cfg"), "keep config");
  return { root, directory, relative: "ghostingmod/runs" };
}

test("ghosts: deleting a run removes only that file and updates the scan", (t) => {
  const f = fixture(t);
  assert.deepEqual(runs.deleteItem(f.root, f.relative, "one.run"), { status: "ok", name: "one.run" });
  assert.deepEqual(runs.scan(f.root, f.relative).items.map((run) => run.name), ["two.run"]);
  assert.equal(fs.readFileSync(path.join(f.directory, "config.cfg"), "utf8"), "keep config");
  assert.equal(fs.readFileSync(path.join(f.root, "outside.run"), "utf8"), "keep outside");
});

test("ghosts: paths outside runs, non-recordings, folders and symlinks cannot be deleted", (t) => {
  const f = fixture(t);
  fs.mkdirSync(path.join(f.directory, "directory.run"));
  for (const name of ["../../outside.run", "..\\..\\outside.run", path.join(f.root, "outside.run"), "config.cfg", "directory.run", null])
    assert.equal(runs.deleteItem(f.root, f.relative, name).status, "error", String(name));
  assert.equal(fs.existsSync(path.join(f.root, "outside.run")), true);
  // Папка runs — ссылка; на Windows это junction, ему права на ссылки не нужны.
  fs.renameSync(f.directory, f.directory + "-real");
  fs.symlinkSync(f.directory + "-real", f.directory, "junction");
  assert.equal(runs.deleteItem(f.root, f.relative, "one.run").status, "error");
  assert.equal(fs.existsSync(path.join(f.directory, "one.run")), true);
  // Ссылка на файл бывает только символической.
  if (NO_SYMLINKS) return t.skip(NO_SYMLINKS);
  const g = fixture(t);
  fs.symlinkSync(path.join(g.root, "outside.run"), path.join(g.directory, "link.run"));
  assert.equal(runs.deleteItem(g.root, g.relative, "link.run").status, "error");
  assert.equal(fs.existsSync(path.join(g.directory, "link.run")), true);
  assert.equal(fs.existsSync(path.join(g.root, "outside.run")), true);
});

test("ghosts: running games prevent deletion; a stopped game permits it", (t) => {
  const f = fixture(t);
  let running = true;
  const context = { runsLocation: () => ({ gameId: "ghosting-renewed", installPath: f.root, relative: f.relative }), service: { isGameRunning: () => running } };
  assert.deepEqual(GameDelivery.prototype.deleteRun.call(context, "ghosting-renewed", "one.run"), { status: "error", code: "GAME_RUNNING" });
  assert.equal(fs.existsSync(path.join(f.directory, "one.run")), true);
  running = false;
  assert.equal(GameDelivery.prototype.deleteRun.call(context, "ghosting-renewed", "one.run").status, "ok");
});

test("HL1 demos: в папке мода видны только .dem, удаляются только они", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-hl1-demos-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const relative = "Half-Life/valve_WON";
  const directory = path.join(root, relative);
  fs.mkdirSync(directory, { recursive: true });
  fs.writeFileSync(path.join(directory, "c1a0.dem"), "demo");
  fs.writeFileSync(path.join(directory, "RUN.DEM"), "demo");
  fs.writeFileSync(path.join(directory, "config.cfg"), "keep config");
  fs.writeFileSync(path.join(directory, "ghost.run"), "not a demo");

  const config = require("../src/main/game-delivery/manifests").localConfigOf("hl1-won");
  assert.equal(config.runsPath, relative);
  assert.equal(config.runsExtension, ".dem");

  const listed = runs.scan(root, relative, ".dem");
  assert.deepEqual(listed.items.map((item) => item.name).sort(), ["RUN.DEM", "c1a0.dem"]);
  assert.equal(runs.deleteItem(root, relative, "config.cfg", ".dem").status, "error");
  assert.equal(runs.deleteItem(root, relative, "ghost.run", ".dem").status, "error");
  assert.equal(runs.resolveItem(root, relative, "ghost.run", ".dem"), null);
  assert.equal(runs.deleteItem(root, relative, "c1a0.dem", ".dem").status, "ok");
  assert.ok(fs.existsSync(path.join(directory, "config.cfg")));

  // Без расширения из конфига — по-прежнему записи Ghosting.
  assert.deepEqual(runs.scan(root, relative).items.map((item) => item.name), ["ghost.run"]);
  // Мусор вместо расширения не расширяет выборку.
  assert.deepEqual(runs.scan(root, relative, "").items.map((item) => item.name), ["ghost.run"]);
});
