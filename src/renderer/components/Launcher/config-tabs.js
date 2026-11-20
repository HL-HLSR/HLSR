"use strict";

// Вкладки редактора конфига (GameConfigEditor): «Файлы cfg», «Проверка» (HL1) и «Игра».
// «Игра» — все понятные настройки одной прокруткой, разделы в порядке каталога
// (shared/game-config: Управление, Изображение, Звук, Игра; пустых у игры нет).
const FILES_ID = "cfg-files";
const CHECK_ID = "cfg-check";
const GAME_ID = "cfg-game";

const L = (ru, en) => ({ ru, en });
const FILES_TAB = { id: FILES_ID, icon: "fa-file-alt", title: L("Файлы cfg", "cfg files") };
const CHECK_TAB = { id: CHECK_ID, icon: "fa-clipboard-check", title: L("Проверка", "Rule check") };
const GAME_TAB = { id: GAME_ID, icon: "fa-sliders-h", title: L("Игра", "Game") };

// catalog — gameConfig.catalogFor() или null; files/check — окно передало редактор файлов и итог проверки.
// fields — настройки вкладки (по ним точка несохранённых правок), sections — её разделы.
function configTabs(catalog, { files = false, check = false } = {}) {
  const tabs = [];
  if (files) tabs.push(Object.assign({ fields: [], sections: [] }, FILES_TAB));
  if (check) tabs.push(Object.assign({ fields: [], sections: [] }, CHECK_TAB));
  const sections = catalog ? catalog.sections : [];
  if (sections.length) tabs.push(Object.assign({ fields: sections.flatMap((section) => section.fields), sections }, GAME_TAB));
  return tabs;
}

module.exports = { configTabs, FILES_ID, CHECK_ID, GAME_ID };
