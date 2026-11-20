"use strict";

const assert = require("node:assert/strict");
const { test } = require("node:test");

const presenceModel = require("../src/main/discord-rpc/presence-model");
const { DiscordRichPresenceBridge } = require("../src/main/discord-rpc");

// Мост без Discord: setPresence только запоминает, что отправлено бы было.
function bridge() {
  const b = new DiscordRichPresenceBridge({ userDataPath: "unused", createClient: () => ({}) });
  const sent = [];
  b.service = { setPresence: (state) => sent.push(state), connected: false };
  b.sent = sent;
  b.last = () => presenceModel.toActivity(sent[sent.length - 1]);
  return b;
}

test("разделы лаунчера: свои состояния, без игры", () => {
  const b = bridge();
  b.onSignal({ type: "selected", gameId: "ghosting-renewed" });
  b.setActivity("personalization");
  assert.equal(b.last().state, "Настраивает игру");
  assert.equal(b.last().details, "HLSR Launcher");
  b.setActivity("demo");
  assert.equal(b.last().state, "Смотрит демки");
  b.setActivity("home");
  assert.equal(b.last().state, "На главной");
  b.setActivity("livesplit");
  assert.equal(b.last().state, "Настраивает LiveSplit");
  b.setActivity("games");
  assert.equal(b.last().details, "Half-Life 2: Ghosting Mod: Renewed");
  b.setActivity("<script>");
  assert.equal(b.activity, null, "неизвестный раздел — как «Игры»");
});

test("рендер демки важнее раздела, установка и игра важнее рендера", () => {
  const b = bridge();
  b.onSignal({ type: "selected", gameId: "hl1-won" });
  b.setActivity("demo");
  b.setDemoRendering(true);
  assert.equal(b.last().state, "Рендерит демку");
  b.onSignal({ type: "status", gameId: "hl1-won", state: "updating" });
  assert.equal(b.last().state, "Обновление сборки");
  assert.equal(b.last().details, "Half-Life (WON)");
  b.onSignal({ type: "status", gameId: "hl1-won", state: "installed" });
  assert.equal(b.last().state, "Рендерит демку");
  b.setDemoRendering(false);
  assert.equal(b.last().state, "Смотрит демки");
});

test("статус другой игры не подменяет выбранную (HL2 Ghosting, а не Half-Life 2 (4260))", () => {
  const b = bridge();
  b.onSignal({ type: "selected", gameId: "ghosting-renewed" });
  b.onSignal({ type: "status", gameId: "ghosting-renewed", state: "installed" });
  b.onSignal({ type: "status", gameId: "hl2-4260", state: "installed" });
  assert.equal(b.last().details, "Half-Life 2: Ghosting Mod: Renewed");
  assert.equal(b.last().state, "Готов к запуску");
  b.onSignal({ type: "status", gameId: "218", state: "updating" });
  assert.equal(b.last().state, "Обновление сборки", "id лаунчера 218 — та же игра");
});

test("эпизод Ghosting: в Discord своё название, после выхода — снова Ghosting", () => {
  const b = bridge();
  b.onSignal({ type: "selected", gameId: "ghosting-renewed" });
  b.onSignal({ type: "running", gameId: "ghosting-renewed", episode: "ep2" });
  assert.equal(b.last().details, "HL2 Ghosting: Episode Two");
  b.setGameState({ active: true, lifecycle: "menu", fps: 300 });
  assert.equal(b.last().details, "Game: HL2 Ghosting: Episode Two");
  assert.equal(b.last().largeImageText, "HL2 Ghosting: Episode Two");
  b.onSignal({ type: "stopped" });
  assert.equal(b.last().details, "Half-Life 2: Ghosting Mod: Renewed");
  b.onSignal({ type: "running", gameId: "ghosting-renewed", episode: "ep1" });
  assert.equal(b.last().details, "HL2 Ghosting: Episode One");
  // Эпизод бывает только у Ghosting; чужое значение — обычное название.
  assert.equal(presenceModel.toActivity({ kind: "playing", gameId: "hl2-4260", episode: "ep1" }).details, "Half-Life 2 (4260)");
  assert.equal(presenceModel.toActivity({ kind: "playing", gameId: "ghosting-renewed", episode: "x" }).details, "Half-Life 2: Ghosting Mod: Renewed");
});

test("sameGame: id лаунчера и id сборки", () => {
  assert.equal(presenceModel.sameGame("218", "ghosting-renewed"), true);
  assert.equal(presenceModel.sameGame(220, "hl2-4260"), true);
  assert.equal(presenceModel.sameGame("hl2-2707", "hl2-4260"), false);
  assert.equal(presenceModel.sameGame("x", "x"), true);
  assert.equal(presenceModel.sameGame(null, "x"), false);
});
