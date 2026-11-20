"use strict";

const CATALOG = {
  LU_SCHEMA_INVALID: {
    ru: "Метаданные обновления повреждены. Обновление отменено.",
    en: "Update metadata is malformed. The update was cancelled.",
  },
  LU_SIGNATURE_INVALID: {
    ru: "Подпись обновления не подтверждена. Обновление отменено.",
    en: "The update signature could not be verified. The update was cancelled.",
  },
  LU_DOWNGRADE: {
    ru: "Предложена версия старее установленной. Обновление отменено.",
    en: "The offered version is older than the installed one. Update cancelled.",
  },
  LU_VERSION_UNSUPPORTED: {
    ru: "Эта версия лаунчера слишком старая: установите свежую вручную.",
    en: "This launcher version is too old: install a fresh build manually.",
  },
  LU_VERSION_MISMATCH: {
    ru: "Версия установщика не совпадает с проверенными метаданными.",
    en: "Installer version does not match the verified metadata.",
  },
  LU_LATEST_YML_MISMATCH: {
    ru: "Файл описания обновления не совпадает с подписанными метаданными.",
    en: "The update description file does not match the signed metadata.",
  },
  LU_INSTALLER_SIZE: {
    ru: "Размер скачанного установщика неверный. Обновление отменено.",
    en: "Downloaded installer has an unexpected size. Update cancelled.",
  },
  LU_INSTALLER_SHA256: {
    ru: "Контрольная сумма установщика не совпала. Обновление отменено.",
    en: "Installer checksum mismatch. Update cancelled.",
  },
  LU_INSTALLER_SHA512: {
    ru: "Контрольная сумма установщика не совпала. Обновление отменено.",
    en: "Installer checksum mismatch. Update cancelled.",
  },
  LU_INSTALLER_UNREADABLE: {
    ru: "Скачанный установщик не читается. Попробуйте ещё раз.",
    en: "The downloaded installer cannot be read. Try again.",
  },
  LU_INSTALLER_TOO_BIG: {
    ru: "Файл обновления неправдоподобно большой. Обновление отменено.",
    en: "The update file is implausibly large. Update cancelled.",
  },
  LU_NO_METADATA: {
    ru: "Нет подтверждённых метаданных обновления.",
    en: "No verified update metadata is available.",
  },
  LU_FEED_UNAVAILABLE: {
    ru: "Сервер обновлений недоступен. Лаунчер продолжит работать.",
    en: "The update server is unavailable. The launcher keeps working.",
  },

  NETWORK_ERROR: {
    ru: "Нет связи с сервером загрузки. Проверьте интернет.",
    en: "Cannot reach the download server. Check your connection.",
  },
  NETWORK_TIMEOUT: {
    ru: "Сервер загрузки не ответил вовремя.",
    en: "The download server timed out.",
  },
  HTTP_ERROR: {
    ru: "Сервер загрузки вернул ошибку.",
    en: "The download server returned an error.",
  },
  INSECURE_REDIRECT: {
    ru: "Загрузка перенаправлена на небезопасный адрес и остановлена.",
    en: "The download was redirected to an insecure address and stopped.",
  },
  CHECKSUM_MISMATCH: {
    ru: "Контрольная сумма файла не совпала: файл не установлен.",
    en: "File checksum mismatch: nothing was installed.",
  },
  SIZE_MISMATCH: {
    ru: "Размер скачанного файла неверный: файл не установлен.",
    en: "Downloaded file has a wrong size: nothing was installed.",
  },
  NO_SPACE: {
    ru: "Недостаточно места на диске.",
    en: "Not enough free disk space.",
  },
  PATH_NOT_WRITABLE: {
    ru: "Нет прав на запись в папку библиотеки.",
    en: "The library folder is not writable.",
  },
  PATH_INVALID: {
    ru: "Путь библиотеки недопустим.",
    en: "The library path is not allowed.",
  },
  ARCHIVE_UNSAFE_ENTRY: {
    ru: "В архиве найден недопустимый путь: установка отменена.",
    en: "The archive contains an unsafe path: installation cancelled.",
  },
  EXTRACT_FAILED: {
    ru: "Не удалось распаковать архив.",
    en: "Failed to extract the archive.",
  },
  INSTALL_FAILED: {
    ru: "Установка не завершена, изменения откачены.",
    en: "Installation failed and was rolled back.",
  },
  REPAIR_FAILED: {
    ru: "Восстановление файлов не завершено.",
    en: "File repair did not complete.",
  },
  VERIFY_FAILED: {
    ru: "Проверка файлов игры выявила расхождения.",
    en: "Game file verification found differences.",
  },
  MANIFEST_INVALID: {
    ru: "Описание игры повреждено или не подписано.",
    en: "The game manifest is malformed or unsigned.",
  },
  CHANNEL_DISABLED: {
    ru: "Загрузка этой игры сейчас отключена.",
    en: "Downloads for this game are currently disabled.",
  },
  CHANNEL_UNAVAILABLE: {
    ru: "Сервер с описанием игры недоступен.",
    en: "The game channel server is unavailable.",
  },
  CANCELLED: {
    ru: "Операция отменена.",
    en: "The operation was cancelled.",
  },

  STEAM_NOT_INSTALLED: {
    ru: "Steam не найден. Установите Steam, чтобы запустить игру.",
    en: "Steam was not found. Install Steam to launch the game.",
  },
  STEAM_NOT_RUNNING: {
    ru: "Steam не запущен. Запустите Steam и повторите.",
    en: "Steam is not running. Start Steam and try again.",
  },
  STEAM_PROCESS_MISMATCH: {
    ru: "Не удалось подтвердить запущенный Steam.",
    en: "The running Steam process could not be confirmed.",
  },
  SDK_NOT_INSTALLED: {
    ru: "Не установлен Source SDK Base 2013 Singleplayer.",
    en: "Source SDK Base 2013 Singleplayer is not installed.",
  },
  ALREADY_RUNNING: {
    ru: "Игра уже запущена.",
    en: "The game is already running.",
  },
  PROCESS_NOT_FOUND: {
    ru: "Процесс игры не найден.",
    en: "The game process was not found.",
  },
  STOP_FAILED: {
    ru: "Не удалось закрыть игру.",
    en: "Failed to close the game.",
  },

  UNINSTALL_REFUSED_PATH: {
    ru: "Папка игр не прошла проверку безопасности и не удалена.",
    en: "The game folder failed safety checks and was not deleted.",
  },
  UNINSTALL_REFUSED_RUNNING: {
    ru: "Игра запущена: очистка остановлена.",
    en: "A game is running: cleanup stopped.",
  },
};

const LANGUAGES = ["ru", "en"];

function isKnownCode(code) {
  return Object.prototype.hasOwnProperty.call(CATALOG, String(code));
}

function messageFor(code, language) {
  const lang = LANGUAGES.indexOf(String(language)) !== -1 ? String(language) : "en";
  const entry = CATALOG[String(code)];

  if (!entry)
    return lang === "ru"
      ? `Непредвиденная ошибка (код ${code}).`
      : `Unexpected error (code ${code}).`;

  return entry[lang];
}

module.exports = { CATALOG, LANGUAGES, isKnownCode, messageFor };
