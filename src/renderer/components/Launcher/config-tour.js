"use strict";

// Шаги обучения по вкладке «Персонализация» (ConfigTour.vue). Здесь только данные:
// какие шаги есть у игры и в каком порядке. Якорь — data-tour="<anchor>" в DOM;
// шаг, чьего якоря нет на экране, ConfigTour пропускает сам.
// pageSection — раздел списка слева, который страница открывает на этом шаге (null — не трогать).

const GHOSTING_ID = "ghosting-renewed";

// Раздел страницы для шага; у выбора игры и импорта с экспортом это «Конфиг», где стоят их кнопки.
const PAGE_SECTION = {
  "configs-games": null,
  "gs-personal": "personal",
  "gs-configs": "configs",
  "configs-import": "configs",
  "configs-export": "configs",
  "gs-keyboard": "keyboard",
  "gs-scripts": "scripts",
  "gs-launch": "launch",
  "gs-demos": "demos",
  "gs-ghosts": "ghosts",
};

// engine: "goldsrc" | "source" | "source-old" (shared/bind-presets), gameId — id сборки ("hl1-won", ...).
function stepsFor(engine, gameId) {
  // Первый шаг подсвечивает саму вкладку «Персонализация» в шапке и не меняет раздел.
  const ids = ["configs-tab", "configs-games"];
  const hasLook = engine === "goldsrc" || gameId === GHOSTING_ID; // внешний вид и демо: HL1 и Ghosting
  if (hasLook) ids.push("gs-personal");
  ids.push("gs-configs", "configs-import", "configs-export", "gs-keyboard");
  if (engine === "goldsrc") ids.push("gs-scripts"); // скрипты гайда есть только у HL1
  ids.push("gs-launch");
  if (hasLook) ids.push("gs-demos");
  if (gameId === GHOSTING_ID) ids.push("gs-ghosts");

  return ids.map((id) => {
    const key = id.toUpperCase().replace(/-/g, "_");
    return {
      id,
      anchor: id,
      section: id === "configs-tab" ? null : "configs",
      pageSection: PAGE_SECTION[id] || null,
      title: `#L_TOUR_${key}_TITLE`,
      text: `#L_TOUR_${key}_TEXT`,
    };
  });
}

module.exports = { stepsFor };
