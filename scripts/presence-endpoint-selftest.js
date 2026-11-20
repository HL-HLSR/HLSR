"use strict";

// Канал связи с игрой переживает перезапуск лаунчера: endpoint-store.js и
// предпочтительный порт PresenceBridgeServer.

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("fs");
const net = require("net");
const os = require("os");
const path = require("path");

const store = require("../src/main/game-presence/endpoint-store");
const { PresenceBridgeServer } = require("../src/main/game-presence/PresenceBridgeServer");

const quiet = { info() {}, warn() {}, error() {} };

function tempFile() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-presence-"));
  return path.join(dir, "presence-endpoint.json");
}

function listening(server) {
  return new Promise((resolve) => {
    const check = () => (server.server.listening ? resolve() : setTimeout(check, 5));
    check();
  });
}

test("канал сохраняется, читается и удаляется", () => {
  const file = tempFile();
  const endpoint = { pipeName: "\\\\.\\pipe\\hlsr-presence-0123456789abcdef", token: "a".repeat(64), port: 40123 };
  assert.equal(store.save(file, endpoint), true);
  assert.deepEqual(store.load(file), endpoint);
  if (process.platform !== "win32") assert.equal(fs.statSync(file).mode & 0o777, 0o600);
  store.clear(file);
  assert.equal(store.load(file), null);
});

test("испорченный файл не принимается", () => {
  const file = tempFile();
  fs.writeFileSync(file, JSON.stringify({ pipeName: "\\\\.\\pipe\\evil", token: "x", port: 1 }));
  assert.equal(store.load(file), null);
  fs.writeFileSync(file, "{не json");
  assert.equal(store.load(file), null);
  assert.equal(store.save(file, { pipeName: "nope", token: "a".repeat(64) }), false);
});

test("перезапуск поднимает тот же канал, токен и порт", { skip: process.platform === "win32" }, async () => {
  const first = new PresenceBridgeServer({ log: quiet, platform: "linux" });
  first.start();
  await listening(first);
  const saved = first.endpoint();
  first.stop();

  const second = new PresenceBridgeServer({ log: quiet, platform: "linux", token: saved.token, pipeName: saved.pipeName, port: saved.port });
  second.start();
  await listening(second);
  assert.deepEqual(second.endpoint(), saved);
  assert.equal(second.environment().HLSR_PRESENCE_TOKEN, saved.token);
  second.stop();
});

test("если старый порт занят, сервер берёт новый", { skip: process.platform === "win32" }, async () => {
  const blocker = net.createServer();
  await new Promise((resolve) => blocker.listen({ host: "127.0.0.1", port: 0 }, resolve));
  const busy = blocker.address().port;

  const server = new PresenceBridgeServer({ log: quiet, platform: "linux", port: busy });
  server.start();
  await listening(server);
  assert.notEqual(server.port(), busy);
  assert.ok(server.port() > 0);
  server.stop();
  blocker.close();
});
