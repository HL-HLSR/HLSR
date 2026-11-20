"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");
const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { execFileSync } = require("child_process");
const { Readable } = require("stream");
const { EventEmitter } = require("events");
const ProtonManager = require("../src/main/linux/ProtonManager");
const ProtonRunner = require("../src/main/linux/ProtonRunner");
const NotificationStore = require("../src/main/notifications/NotificationStore");
const { NO_SYMLINKS, TAR } = require("./lib/selftest-caps");

// ProtonManager и фикстура зовут tar из PATH; в Git Bash первым стоит GNU tar, который не понимает
// пути «C:\…». Впереди — системный bsdtar, как у процесса Windows вне Git Bash.
if (process.platform === "win32") process.env.PATH = path.dirname(TAR) + path.delimiter + process.env.PATH;

function writeProton(directory) {
  fs.mkdirSync(directory, { recursive: true });
  fs.writeFileSync(path.join(directory, "proton"), "#!/bin/sh\nexit 0\n", { mode: 0o755 });
}

function fixture(t, options = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-proton-update-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const old = path.join(root, "protons", ProtonManager.PINNED_ARCHIVE.name.replace(/\.tar\.xz$/, ""));
  writeProton(old);
  // Сборка новее закреплённой: обновление должно быть именно обновлением.
  const tag = "wineland-11.0-20991231-hlsr99";
  const name = `proton-${tag}-x86_64`;
  const source = path.join(root, "source");
  writeProton(path.join(source, name));
  const file = path.join(root, `${name}.tar.xz`);
  execFileSync("tar", ["-cJf", file, "-C", source, name]);
  const content = fs.readFileSync(file);
  const sha256 = crypto.createHash("sha256").update(content).digest("hex");
  const sha512 = crypto.createHash("sha512").update(content).digest("hex");
  const archive = { name: `${name}.tar.xz`, size: content.length, digest: `sha256:${sha256}`,
    browser_download_url: "https://github.com/test/archive" };
  const sums = { name: `${name}.sha512sum`, browser_download_url: "https://github.com/test/sums" };
  const release = { tag_name: tag, assets: [archive, sums] };
  const reads = [];
  let downloads = 0;
  const manager = new ProtonManager({ dataDir: root, home: root, inUse: () => false,
    readText: async (url) => {
      reads.push(url);
      return url === ProtonManager.RELEASES_API ? JSON.stringify([release]) : `${sha512}  ${archive.name}\n`;
    },
    request: async () => { downloads++; return Readable.from([content]); },
    ...options,
  });
  manager.select(old);
  return { root, old, target: path.join(root, "protons", name), manager, reads, tag,
    release, content, sha512, downloads: () => downloads };
}

test("Proton: startup checks metadata once and offers an update without downloading or changing selection", async (t) => {
  const f = fixture(t);
  const offers = [];
  f.manager.on("update-available", (event) => offers.push(event));
  await f.manager.ensureReference();
  await Promise.all([f.manager.checkForUpdate(), f.manager.checkForUpdate()]);
  assert.deepEqual(f.reads, [ProtonManager.RELEASES_API]);
  assert.equal(f.downloads(), 0);
  assert.equal(f.manager.selected().path, f.old);
  assert.equal(fs.existsSync(f.target), false);
  assert.equal(f.manager.status().update.available, true);
  assert.deepEqual(offers, [{ from: ProtonManager.RELEASE_TAG, to: f.tag }]);
});

test("Proton: explicit update installs a verified archive once and preserves the old build and prefix links", { skip: NO_SYMLINKS }, async (t) => {
  const f = fixture(t);
  const prefix = path.join(f.root, "compatdata/test/pfx/drive_c/windows");
  fs.mkdirSync(prefix, { recursive: true });
  fs.symlinkSync(path.join(f.old, "proton"), path.join(prefix, "test.exe"));
  const events = [];
  f.manager.on("updated", (event) => events.push(event));
  await Promise.all([f.manager.applyUpdate(), f.manager.applyUpdate()]);
  assert.equal(f.downloads(), 1);
  assert.equal(f.manager.selected().path, f.target);
  assert.equal(fs.existsSync(path.join(f.old, "proton")), true);
  assert.equal(fs.existsSync(path.join(prefix, "test.exe")), true);
  assert.equal(f.manager.status().update.available, false);
  assert.equal(f.manager.status().phase, "ready");
  assert.equal(events.length, 1);
  const restarted = new ProtonManager({ dataDir: f.root, home: f.root });
  await restarted.ensureReference();
  assert.equal(restarted.selected().path, f.target);
  restarted.select(f.old);
  assert.equal(restarted.selected().path, f.old, "ручной откат доступен");
});

test("Proton: first automatic installation announces start and completion once", async (t) => {
  const f = fixture(t);
  fs.rmSync(f.old, { recursive: true });
  const events = [];
  f.manager.on("install-started", () => events.push("start"));
  f.manager.on("installed", () => events.push("done"));
  f.manager.readText = async () => JSON.stringify({ tag_name: ProtonManager.RELEASE_TAG,
    assets: [{ ...ProtonManager.PINNED_ARCHIVE, browser_download_url: "https://github.com/test/pinned" }] });
  f.manager.installArchive = async (archive, expected) => {
    assert.deepEqual(expected, ProtonManager.PINNED_ARCHIVE);
    writeProton(f.old);
    return f.manager.discover().find((entry) => entry.path === f.old);
  };
  await Promise.all([f.manager.ensureReference(), f.manager.ensureReference()]);
  assert.deepEqual(events, ["start", "done"]);
  assert.equal(f.manager.state.operation, "install");
  await f.manager.ensureReference();
  assert.deepEqual(events, ["start", "done"]);
});

test("Proton: a corrupted update preserves selection, clears staging and can be retried", async (t) => {
  const f = fixture(t);
  f.manager.request = async () => Readable.from([Buffer.from("corrupt")]);
  await assert.rejects(f.manager.applyUpdate(), /не совпал/);
  assert.equal(f.manager.selected().path, f.old);
  assert.equal(f.manager.pending, null);
  assert.equal(f.manager.controller, null);
  assert.equal(f.manager.state.phase, "error");
  assert.equal(f.manager.update.available, true);
  assert.deepEqual(fs.readdirSync(f.manager.root), [path.basename(f.old)]);
  f.manager.request = async () => Readable.from([f.content]);
  await f.manager.applyUpdate();
  assert.equal(f.manager.selected().path, f.target);
});

test("Proton: cancellation while reading update checksums preserves the offer and supports retry", async (t) => {
  const f = fixture(t);
  await f.manager.checkForUpdate();
  const original = f.manager.readText;
  let started;
  const ready = new Promise((resolve) => { started = resolve; });
  f.manager.readText = (url, limit, signal) => {
    started();
    return new Promise((resolve, reject) => signal.addEventListener("abort", () => reject(signal.reason), { once: true }));
  };
  const pending = f.manager.applyUpdate();
  await ready;
  assert.equal(f.manager.cancel(), true);
  assert.equal(await pending, null);
  assert.equal(f.manager.state.phase, "idle");
  assert.equal(f.manager.pending, null);
  assert.equal(f.manager.controller, null);
  assert.equal(f.manager.selected().path, f.old);
  assert.equal(f.manager.update.available, true);
  f.manager.readText = original;
  await f.manager.applyUpdate();
  assert.equal(f.manager.selected().path, f.target);
});

test("Proton: running Wine blocks switching before and during an update, retry reuses the downloaded build", async (t) => {
  let busy = true;
  const f = fixture(t, { inUse: () => busy });
  await assert.rejects(f.manager.applyUpdate(), (error) => error.code === "PROTON_IN_USE");
  assert.equal(f.downloads(), 0);
  busy = false;
  const original = f.manager.readText;
  f.manager.readText = async (...args) => { busy = true; return original(...args); };
  await assert.rejects(f.manager.applyUpdate(), (error) => error.code === "PROTON_IN_USE");
  assert.equal(f.manager.selected().path, f.old);
  assert.equal(fs.existsSync(f.target), true);
  assert.equal(f.manager.state.phase, "error");
  busy = false;
  await f.manager.applyUpdate();
  assert.equal(f.downloads(), 1);
  assert.equal(f.manager.selected().path, f.target);
});

test("Proton: update preserves a custom selection", async (t) => {
  const f = fixture(t);
  const custom = path.join(f.root, "custom/GE-Proton");
  writeProton(custom);
  f.manager.addCustom(custom);
  f.manager.select(custom);
  await f.manager.applyUpdate();
  assert.equal(f.manager.selected().path, custom);
  assert.equal(fs.existsSync(f.target), true);
  assert.equal(fs.existsSync(f.old), true);
});

test("Proton: update notification preserves its action target across storage", (t) => {
  const f = fixture(t);
  const notifications = new NotificationStore({ file: path.join(f.root, "notifications.json") });
  notifications.add({ level: "info", code: "PROTON_UPDATE_AVAILABLE",
    meta: { target: "proton", newVersion: f.tag } });
  assert.deepEqual(notifications.list().items[0].meta, { target: "proton", newVersion: f.tag });
});

test("Proton: stale prefix links are prepared before launch and shared across concurrent preparation", { skip: NO_SYMLINKS }, async (t) => {
  const f = fixture(t);
  writeProton(f.target);
  f.manager.select(f.target);
  fs.rmSync(f.old, { recursive: true });
  const prefix = path.join(f.root, "compatdata/test/pfx");
  const builtin = path.join(prefix, "drive_c/windows/system32/kernel32.dll");
  const replacement = path.join(f.target, "files/lib/wine/x86_64-windows/kernel32.dll");
  fs.mkdirSync(path.dirname(builtin), { recursive: true });
  fs.mkdirSync(path.dirname(replacement), { recursive: true });
  fs.writeFileSync(replacement, "Wine builtin DLL");
  fs.symlinkSync(path.join(f.old, "files/lib/wine/x86_64-windows/kernel32.dll"), builtin);
  const saved = path.join(prefix, "drive_c/save.dat");
  fs.writeFileSync(saved, "keep save");
  const unrelated = path.join(prefix, "drive_c/unrelated");
  fs.symlinkSync("/missing/user/file", unrelated);
  const obsolete = path.join(prefix, "drive_c/obsolete.dll");
  fs.symlinkSync(path.join(f.old, "files/lib/wine/x86_64-windows/opencl.dll"), obsolete);
  const game = path.join(f.root, "game.exe");
  fs.writeFileSync(game, "test executable");
  const calls = [];
  const runner = new ProtonRunner({ manager: f.manager, dataDir: f.root, resourcesPath: f.root,
    pythonRuntime: { prepareEnvironment: async (env) => env },
    userNamespaces: { ensure: async () => {} },
    spawnFn: (command, args) => {
      calls.push(args);
      const child = new EventEmitter();
      child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
      setImmediate(() => {
        child.emit("spawn");
        if (args[0] === "wineboot.exe") {
          fs.unlinkSync(builtin); fs.symlinkSync(replacement, builtin);
          child.emit("close", 0, null);
        }
      });
      return child;
    },
  });
  runner.umuPath = () => "/test/umu-run";
  await Promise.all([runner.ensurePrefixRuntime("test"), runner.ensurePrefixRuntime("test")]);
  await runner.launch("test", game, [], { skipFonts: true });
  await runner.launch("test", game, [], { skipFonts: true });
  assert.deepEqual(calls, [["wineboot.exe", "-u"], [game], [game]]);
  assert.equal(fs.existsSync(builtin), true);
  assert.equal(fs.readFileSync(saved, "utf8"), "keep save");
  assert.equal(fs.readlinkSync(unrelated), "/missing/user/file");
  assert.equal(fs.readlinkSync(obsolete), path.join(f.old, "files/lib/wine/x86_64-windows/opencl.dll"));
  assert.equal(runner.prefixPending.size, 0);
});

test("Proton: wineboot -u под чужим Proton не повторяется, пока префикс не менялся", { skip: NO_SYMLINKS }, async (t) => {
  const f = fixture(t);
  const experimental = path.join(f.root, ".local/share/Steam/steamapps/common/Proton - Experimental");
  writeProton(experimental);
  fs.writeFileSync(path.join(experimental, "version"), "1791000000 experimental-11.0\n");
  const relative = "files/lib/wine/x86_64-windows/explorer.exe";
  fs.mkdirSync(path.dirname(path.join(experimental, relative)), { recursive: true });
  fs.writeFileSync(path.join(experimental, relative), "Wine builtin");
  f.manager.select(experimental);
  const prefix = path.join(f.root, "compatdata/test/pfx");
  fs.mkdirSync(path.join(prefix, "drive_c/windows"), { recursive: true });
  // Ссылку в корне C:\windows wineboot чужого Proton не переписывает.
  fs.symlinkSync(path.join(f.old, relative), path.join(prefix, "drive_c/windows/explorer.exe"));
  fs.writeFileSync(path.join(prefix, "version"), "experimental-11.0\n");
  const game = path.join(f.root, "game.exe");
  fs.writeFileSync(game, "test executable");
  const calls = [];
  const runner = new ProtonRunner({ manager: f.manager, dataDir: f.root, resourcesPath: f.root,
    pythonRuntime: { prepareEnvironment: async (env) => env },
    userNamespaces: { ensure: async () => {} },
    spawnFn: (command, args) => {
      calls.push(args[0]);
      const child = new EventEmitter();
      child.stdout = new EventEmitter(); child.stderr = new EventEmitter();
      setImmediate(() => { child.emit("spawn"); if (args[0] === "wineboot.exe") child.emit("close", 0, null); });
      return child;
    },
  });
  runner.umuPath = () => "/test/umu-run";
  await runner.launch("test", game, [], { skipFonts: true });
  await runner.launch("test", game, [], { skipFonts: true });
  assert.deepEqual(calls, ["wineboot.exe", game, game]);

  // Префикс обновил другой Proton: отметка больше не действует.
  fs.writeFileSync(path.join(prefix, "version"), "wineland-11.0\n");
  calls.length = 0;
  await runner.launch("test", game, [], { skipFonts: true });
  assert.deepEqual(calls, ["wineboot.exe", game]);
});

test("Proton: failed prefix preparation prevents game launch and permits retry", { skip: NO_SYMLINKS }, async (t) => {
  const f = fixture(t);
  const prefix = path.join(f.root, "compatdata/test/pfx/drive_c");
  fs.mkdirSync(prefix, { recursive: true });
  const link = path.join(prefix, "old.dll");
  fs.symlinkSync(path.join(f.target, "missing.dll"), link);
  fs.writeFileSync(path.join(f.old, "missing.dll"), "Wine builtin DLL");
  const game = path.join(f.root, "game.exe");
  fs.writeFileSync(game, "test");
  const runner = new ProtonRunner({ manager: f.manager, dataDir: f.root, resourcesPath: f.root });
  let preparations = 0;
  runner.runPrefixCommand = async () => { preparations++; throw new Error("wineboot failed"); };
  for (let attempt = 0; attempt < 2; attempt++)
    await assert.rejects(runner.launch("test", game, [], { skipFonts: true }), /wineboot failed/);
  assert.equal(preparations, 2);
  assert.equal(runner.prefixPending.size, 0);
  assert.equal(fs.readFileSync(game, "utf8"), "test");
});
