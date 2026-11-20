"use strict";

// «Повтори цвета» на шариках LiveSplit (секретное достижение good-memory).
// Игра — { sequence: номера шариков, step: сколько из них игрок уже повторил }.
// Каждый раунд добавляет один случайный цвет, ошибка сбрасывает игру,
// восьмой раунд без ошибки — победа (владелец 11.11).

const ROUNDS = 8;

function pick(count, random) {
  return Math.min(count - 1, Math.floor(random() * count));
}

function start(count, random = Math.random) {
  return { sequence: [pick(count, random)], step: 0 };
}

// Щелчок по шарику index. result: "wrong" — сброс (game: null), "ok" — ждём
// следующий щелчок, "round" — раунд пройден и в sequence уже новый цвет,
// "won" — пройден последний раунд (game: null).
function press(game, index, count, random = Math.random) {
  if (!game || game.sequence[game.step] !== index) return { game: null, result: "wrong" };
  const step = game.step + 1;
  if (step < game.sequence.length) return { game: { sequence: game.sequence, step }, result: "ok" };
  if (game.sequence.length >= ROUNDS) return { game: null, result: "won" };
  return { game: { sequence: game.sequence.concat(pick(count, random)), step: 0 }, result: "round" };
}

module.exports = { ROUNDS, start, press };
