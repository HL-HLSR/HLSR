"use strict";

// Чипы игр на странице «Персонализация». Half-Life 2 («220») один: установлен — обычный чип
// с пометкой «в разработке»; не установлен — неактивный чип. Остальные игры — как есть.
// Последним — неактивный LiveSplit «в разработке»: своей персонализации у него пока нет.
const HL2_ID = "220";

function configChips(games) {
  const chips = games.map((game) => ({ id: game.id, name: game.id === HL2_ID ? "Half-Life 2" : game.name, soon: game.id === HL2_ID, disabled: false }));
  if (!games.some((game) => game.id === HL2_ID)) chips.push({ id: "", name: "Half-Life 2", soon: true, disabled: true });
  chips.push({ id: "", name: "LiveSplit", soon: true, disabled: true });
  return chips;
}

module.exports = { configChips, HL2_ID };
