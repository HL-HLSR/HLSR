"use strict";

const AVAILABLE_GAME_IDS = ["218", "220", "70"];

const DEFAULT_GAME_ID = "218";

const GAME_ORDER = ["218", "220", "70"];

// Эпизоды HL2 не отдельные игры в колонке: они готовятся как сборки
// Half-Life 2 и показаны в её списке сборок с пометкой «скоро».
const HIDDEN_GAME_IDS = ["50", "130", "380", "420"];

function isGameAvailable(id) {
  return AVAILABLE_GAME_IDS.indexOf(String(id)) !== -1;
}

function isGameHidden(id) {
  return HIDDEN_GAME_IDS.indexOf(String(id)) !== -1;
}

function orderGames(games) {
  const byId = new Map(
    games.filter((game) => !isGameHidden(game.id)).map((game) => [String(game.id), game])
  );
  const ordered = [];

  for (const id of GAME_ORDER) {
    const game = byId.get(id);
    if (game) {
      ordered.push(game);
      byId.delete(id);
    }
  }

  for (const game of byId.values()) ordered.push(game);

  return ordered;
}

function resolveStartupGame(savedId) {
  if (savedId != null && isGameAvailable(savedId)) return String(savedId);
  return DEFAULT_GAME_ID;
}

module.exports = {
  AVAILABLE_GAME_IDS,
  DEFAULT_GAME_ID,
  GAME_ORDER,
  HIDDEN_GAME_IDS,
  isGameAvailable,
  isGameHidden,
  orderGames,
  resolveStartupGame,
};
