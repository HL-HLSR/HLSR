"use strict";

// Сетевые ошибки Chromium, когда не отвечает прокси из настроек системы:
// Electron ходит в сеть через системный прокси, и если программу-VPN закрыли,
// а прокси в Windows остался включён, любой запрос лаунчера (обновления,
// загрузки игр, API) падает с net::ERR_PROXY_CONNECTION_FAILED и т. п.
// Голый код человеку ничего не говорит — центр уведомлений
// (NotificationCenter.vue) показывает вместо него подсказку, что делать.
const PROXY_ERROR = /\bERR_(?:PROXY_[A-Z0-9_]+|TUNNEL_CONNECTION_FAILED|MANDATORY_PROXY_CONFIGURATION_FAILED)\b/;

function isProxyError(text) {
  return typeof text === "string" && PROXY_ERROR.test(text);
}

module.exports = { isProxyError };
