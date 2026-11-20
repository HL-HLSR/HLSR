"use strict";

// episode-button.js: кнопка карточки Ghosting при выбранном эпизоде в «Версии сборки».
// node --test scripts/episode-button-selftest.js

const assert = require("node:assert/strict");
const test = require("node:test");
const { episodeButton } = require("../src/shared/episode-button");
const mainButton = require("../src/main/game-delivery/main-button");

const READY = { state: "ready", action: "play", enabled: true, spinner: false, progress: false };
const ghosting = (episodes, extra) => Object.assign({ installedVersion: "1.0.13", operation: null, episodes }, extra);
const BOTH_MISSING = { ep1: { installed: false, paused: false }, ep2: { installed: false, paused: false } };

test("без эпизода — кнопка Ghosting как есть", () => {
  assert.equal(episodeButton(READY, ghosting(BOTH_MISSING), null), READY);
});

test("Ghosting не установлена — «Установить» ставит Ghosting, а не эпизод", () => {
  const install = mainButton.resolve({ delivery: { state: "not-installed", installable: true } });
  assert.equal(install.state, "not-installed");
  assert.equal(episodeButton(install, { installedVersion: null, episodes: BOTH_MISSING }, "ep1"), install);
});

test("обновление, ремонт, проверка обновлений, недоступность, ошибка — кнопка Ghosting, хоть эпизоды и не скачаны", () => {
  const update = mainButton.resolve({ delivery: { installedVersion: "1.0.13" }, update: { updateAvailable: true, availableVersion: "1.0.14" } });
  assert.equal(update.state, "update-available");
  assert.equal(episodeButton(update, ghosting(BOTH_MISSING), "ep1"), update);

  const repair = mainButton.resolve({ delivery: { installedVersion: "1.0.13", state: "files-damaged", integrity: { damaged: 2 } } });
  assert.equal(repair.state, "repair-required");
  assert.equal(episodeButton(repair, ghosting(BOTH_MISSING), "ep2"), repair);

  for (const state of ["checking-updates", "unavailable", "error", "update-error", "update-launcher-required"]) {
    const button = { state, action: "none", enabled: false };
    assert.equal(episodeButton(button, ghosting(BOTH_MISSING), "ep1"), button, state);
  }
});

test("идёт операция эпизода или другая — кнопка Ghosting (прогресс, пауза)", () => {
  const downloading = { state: "downloading", action: "pause", enabled: true, progress: true };
  for (const operation of ["episode-install", "episode-remove", "update"])
    assert.equal(episodeButton(downloading, ghosting(BOTH_MISSING, { operation }), "ep2"), downloading, operation);
});

test("игра запускается, идёт или закрывается, main занят — кнопка Ghosting", () => {
  for (const state of ["starting-game", "game-running", "stopping-game", "busy", "installing", "downloading"]) {
    const button = { state, action: "none", enabled: false };
    assert.equal(episodeButton(button, ghosting(BOTH_MISSING), "ep1"), button, state);
  }
});

test("загрузка эпизода на паузе — «Продолжить»", () => {
  const button = episodeButton(READY, ghosting({ ep1: { installed: false, paused: true } }), "ep1");
  assert.deepEqual(button, { state: "episode-paused", action: "resume", enabled: true, spinner: false, progress: false });
});

test("эпизод не скачан — «Скачать эпизод»", () => {
  assert.deepEqual(episodeButton(READY, ghosting(BOTH_MISSING), "ep2"), {
    state: "episode-not-installed",
    action: "install",
    enabled: true,
    spinner: false,
    progress: false,
  });
  assert.equal(episodeButton(READY, ghosting(undefined), "ep1").state, "episode-not-installed", "статус без episodes");
});

test("эпизод установлен — кнопка Ghosting: «Играть», «Обновить»", () => {
  const installed = ghosting({ ep1: { installed: true, version: "1.0.0", paused: false } });
  assert.equal(episodeButton(READY, installed, "ep1"), READY);
  const update = { state: "update-available", action: "update", enabled: true };
  assert.equal(episodeButton(update, installed, "ep1"), update);
});
