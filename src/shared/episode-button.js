"use strict";

// Кнопка карточки Ghosting, когда в «Версии сборки» выбран эпизод (Episode One/Two).
// Эпизод подменяет только «Играть» установленной Ghosting: всё остальное (установка,
// обновление, ремонт, ошибки, загрузка, запущенная игра) — кнопка Ghosting (main-button.js).

function view(state, action) {
  return { state, action, enabled: true, spinner: false, progress: false };
}

function episodeButton(mainButton, delivery, episodeId) {
  if (!episodeId || !mainButton || mainButton.state !== "ready") return mainButton;

  const episode = ((delivery && delivery.episodes) || {})[episodeId] || {};
  if (episode.paused) return view("episode-paused", "resume");
  if (!episode.installed) return view("episode-not-installed", "install");
  return mainButton;
}

module.exports = { episodeButton };
