"use strict";

// Официальная сборка HLSR: при сборке выпуска вшиты ключ аттестации и адрес онлайна
// (.electron-vue/webpack.main.config.js). Самосборка из исходников их не получает: онлайн,
// вход через Steam, «Главная», профили, друзья, спонсорство и Telegram в ней выключены, и к
// серверу HLSR она не обращается. Сервер со своей стороны тоже не пускает неофициальные сборки.
const config = require("../telemetry/config");
const BuildKey = require("./build-key");

function isOfficial() {
  return config.BACKEND_CONFIGURED && BuildKey.embedded().available();
}

module.exports = { isOfficial };
