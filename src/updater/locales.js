"use strict";

const STRINGS = {
  ru: {
    title: "Обновление HLSR",
    subtitle: "Версия %0%",
    stageDownload: "Скачивание обновления…",
    stageVerify: "Проверка",
    stageWaitExit: "Ожидание закрытия HLSR…",
    stageInstall: "Установка",
    stageRelaunch: "Запуск HLSR",
    stageDone: "Готово",
    cancel: "Отменить",
    close: "Закрыть",
    retry: "Повторить",
    cancelled: "Обновление отменено",
    cancelledHint: "Лаунчер остался прежней версии. Обновиться можно позже.",
    done: "HLSR обновлён до версии %0%",
    doneHint: "Лаунчер запускается…",
    errorTitle: "Не удалось обновить HLSR",
    errorHint: "Прежняя рабочая версия осталась на месте.",
    of: "из",
    perSecond: "/с",
    waitingHlsr: "HLSR ещё работает. Закройте лаунчер, чтобы продолжить.",

    UPD_ALREADY_RUNNING: "Обновление уже выполняется в другом окне.",
    UPD_BAD_PLAN: "Некорректные данные обновления.",
    UPD_DOWNLOAD_FAILED: "Не удалось скачать обновление. Проверьте подключение к интернету.",
    UPD_DOWNLOAD_HTTP: "Сервер обновлений вернул ошибку.",
    UPD_DOWNLOAD_TIMEOUT: "Сервер обновлений не отвечает.",
    UPD_DOWNLOAD_SIZE: "Размер загруженного файла не совпал с ожидаемым.",
    UPD_NO_SPACE: "Недостаточно места на диске для обновления.",
    UPD_URL_NOT_ALLOWED: "Адрес загрузки не входит в список доверенных.",
    UPD_FILENAME_MISMATCH: "Имя файла обновления не совпадает с подписанным.",
    UPD_VERSION_MISMATCH: "Версия обновления не совпадает с подписанной.",
    UPD_SIGNATURE_INVALID: "Подпись обновления недействительна. Установка отменена.",
    UPD_HLSR_STILL_RUNNING: "HLSR не закрылся. Закройте лаунчер и повторите обновление.",
    UPD_INSTALL_FAILED: "Установщик не смог завершить установку.",
    UPD_INSTALL_EXIT_CODE: "Установщик завершился с ошибкой.",
    UPD_INSTALL_NOT_APPLIED: "Обновление не применилось.",
    UPD_RELAUNCH_FAILED: "Не удалось запустить обновлённый HLSR.",
    LU_INSTALLER_SHA256: "Контрольная сумма обновления не совпала. Файл повреждён.",
    LU_INSTALLER_SHA512: "Контрольная сумма обновления не совпала. Файл повреждён.",
    LU_INSTALLER_SIZE: "Размер обновления не совпал с подписанным.",
    LU_SIGNATURE_INVALID: "Подпись обновления недействительна. Установка отменена.",
    LU_SCHEMA_INVALID: "Данные обновления повреждены.",
  },

  en: {
    title: "HLSR Update",
    subtitle: "Version %0%",
    stageDownload: "Downloading update…",
    stageVerify: "Verifying",
    stageWaitExit: "Waiting for HLSR to close…",
    stageInstall: "Installing",
    stageRelaunch: "Starting HLSR",
    stageDone: "Done",
    cancel: "Cancel",
    close: "Close",
    retry: "Retry",
    cancelled: "Update cancelled",
    cancelledHint: "The launcher was left on the current version. You can update later.",
    done: "HLSR updated to version %0%",
    doneHint: "Starting the launcher…",
    errorTitle: "Could not update HLSR",
    errorHint: "The previous working version is still installed and unchanged.",
    of: "of",
    perSecond: "/s",
    waitingHlsr: "HLSR is still running. Close the launcher to continue.",

    UPD_ALREADY_RUNNING: "An update is already running in another window.",
    UPD_BAD_PLAN: "Invalid update data.",
    UPD_DOWNLOAD_FAILED: "Could not download the update. Check your internet connection.",
    UPD_DOWNLOAD_HTTP: "The update server returned an error.",
    UPD_DOWNLOAD_TIMEOUT: "The update server is not responding.",
    UPD_DOWNLOAD_SIZE: "The downloaded file size did not match.",
    UPD_NO_SPACE: "Not enough disk space for the update.",
    UPD_URL_NOT_ALLOWED: "The download address is not on the trusted list.",
    UPD_FILENAME_MISMATCH: "The update file name does not match the signed one.",
    UPD_VERSION_MISMATCH: "The update version does not match the signed one.",
    UPD_SIGNATURE_INVALID: "The update signature is invalid. Installation cancelled.",
    UPD_HLSR_STILL_RUNNING: "HLSR did not close. Close the launcher and try again.",
    UPD_INSTALL_FAILED: "The installer could not finish.",
    UPD_INSTALL_EXIT_CODE: "The installer exited with an error.",
    UPD_INSTALL_NOT_APPLIED: "The update was not applied.",
    UPD_RELAUNCH_FAILED: "Could not start the updated HLSR.",
    LU_INSTALLER_SHA256: "The update checksum did not match. The file is corrupted.",
    LU_INSTALLER_SHA512: "The update checksum did not match. The file is corrupted.",
    LU_INSTALLER_SIZE: "The update size did not match the signed one.",
    LU_SIGNATURE_INVALID: "The update signature is invalid. Installation cancelled.",
    LU_SCHEMA_INVALID: "The update data is corrupted.",
  },
};

function pack(language) {
  return STRINGS[language === "en" ? "en" : "ru"];
}

function errorText(language, code) {
  const strings = pack(language);
  return strings[code] || strings.errorTitle;
}

module.exports = { STRINGS, pack, errorText };
