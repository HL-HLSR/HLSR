"use strict";
const $ = (id) => document.getElementById(id);
document.addEventListener("wheel", (event) => {
  if (event.ctrlKey || event.metaKey) event.preventDefault();
}, { passive: false });

// Значки Font Awesome 7 Free (solid, CC BY 4.0), те же, что в лаунчере.
// Здесь только контуры: шрифт значков в установщик не кладётся, а CSP
// окна не пускает ничего, кроме своих файлов. [ширина viewBox, контур];
// высота у всех 512.
const ICONS = {
  download: [448, "M256 32c0-17.7-14.3-32-32-32s-32 14.3-32 32l0 210.7-41.4-41.4c-12.5-12.5-32.8-12.5-45.3 0s-12.5 32.8 0 45.3l96 96c12.5 12.5 32.8 12.5 45.3 0l96-96c12.5-12.5 12.5-32.8 0-45.3s-32.8-12.5-45.3 0L256 242.7 256 32zM64 320c-35.3 0-64 28.7-64 64l0 32c0 35.3 28.7 64 64 64l320 0c35.3 0 64-28.7 64-64l0-32c0-35.3-28.7-64-64-64l-46.9 0-56.6 56.6c-31.2 31.2-81.9 31.2-113.1 0L110.9 320 64 320zm304 56a24 24 0 1 1 0 48 24 24 0 1 1 0-48z"],
  update: [512, "M65.9 228.5c13.3-93 93.4-164.5 190.1-164.5 53 0 101 21.5 135.8 56.2 .2 .2 .4 .4 .6 .6l7.6 7.2-47.9 0c-17.7 0-32 14.3-32 32s14.3 32 32 32l128 0c17.7 0 32-14.3 32-32l0-128c0-17.7-14.3-32-32-32s-32 14.3-32 32l0 53.4-11.3-10.7C390.5 28.6 326.5 0 256 0 127 0 20.3 95.4 2.6 219.5 .1 237 12.2 253.2 29.7 255.7s33.7-9.7 36.2-27.1zm443.5 64c2.5-17.5-9.7-33.7-27.1-36.2s-33.7 9.7-36.2 27.1c-13.3 93-93.4 164.5-190.1 164.5-53 0-101-21.5-135.8-56.2-.2-.2-.4-.4-.6-.6l-7.6-7.2 47.9 0c17.7 0 32-14.3 32-32s-14.3-32-32-32L32 320c-8.5 0-16.7 3.4-22.7 9.5S-.1 343.7 0 352.3l1 127c.1 17.7 14.6 31.9 32.3 31.7S65.2 496.4 65 478.7l-.4-51.5 10.7 10.1c46.3 46.1 110.2 74.7 180.7 74.7 129 0 235.7-95.4 253.4-219.5z"],
  retry: [512, "M488 192l-144 0c-9.7 0-18.5-5.8-22.2-14.8s-1.7-19.3 5.2-26.2l46.7-46.7c-75.3-58.6-184.3-53.3-253.5 15.9-75 75-75 196.5 0 271.5s196.5 75 271.5 0c8.2-8.2 15.5-16.9 21.9-26.1 10.1-14.5 30.1-18 44.6-7.9s18 30.1 7.9 44.6c-8.5 12.2-18.2 23.8-29.1 34.7-100 100-262.1 100-362 0S-25 175 75 75c94.3-94.3 243.7-99.6 344.3-16.2L471 7c6.9-6.9 17.2-8.9 26.2-5.2S512 14.3 512 24l0 144c0 13.3-10.7 24-24 24z"],
  open: [512, "M320 0c-17.7 0-32 14.3-32 32s14.3 32 32 32l82.7 0-201.4 201.4c-12.5 12.5-12.5 32.8 0 45.3s32.8 12.5 45.3 0L448 109.3 448 192c0 17.7 14.3 32 32 32s32-14.3 32-32l0-160c0-17.7-14.3-32-32-32L320 0zM80 96C35.8 96 0 131.8 0 176L0 432c0 44.2 35.8 80 80 80l256 0c44.2 0 80-35.8 80-80l0-80c0-17.7-14.3-32-32-32s-32 14.3-32 32l0 80c0 8.8-7.2 16-16 16L80 448c-8.8 0-16-7.2-16-16l0-256c0-8.8 7.2-16 16-16l80 0c17.7 0 32-14.3 32-32s-14.3-32-32-32L80 96z"],
  back: [512, "M9.4 233.4c-12.5 12.5-12.5 32.8 0 45.3l160 160c12.5 12.5 32.8 12.5 45.3 0s12.5-32.8 0-45.3L109.3 288 480 288c17.7 0 32-14.3 32-32s-14.3-32-32-32l-370.7 0 105.4-105.4c12.5-12.5 12.5-32.8 0-45.3s-32.8-12.5-45.3 0l-160 160z"],
  arrowRight: [512, "M502.6 278.6c12.5-12.5 12.5-32.8 0-45.3l-160-160c-12.5-12.5-32.8-12.5-45.3 0s-12.5 32.8 0 45.3L402.7 224 32 224c-17.7 0-32 14.3-32 32s14.3 32 32 32l370.7 0-105.4 105.4c-12.5 12.5-12.5 32.8 0 45.3s32.8 12.5 45.3 0l160-160z"],
  cancel: [384, "M55.1 73.4c-12.5-12.5-32.8-12.5-45.3 0s-12.5 32.8 0 45.3L147.2 256 9.9 393.4c-12.5 12.5-12.5 32.8 0 45.3s32.8 12.5 45.3 0L192.5 301.3 329.9 438.6c12.5 12.5 32.8 12.5 45.3 0s12.5-32.8 0-45.3L237.8 256 375.1 118.6c12.5-12.5 12.5-32.8 0-45.3s-32.8-12.5-45.3 0L192.5 210.7 55.1 73.4z"],
  minimize: [448, "M0 256c0-17.7 14.3-32 32-32l384 0c17.7 0 32 14.3 32 32s-14.3 32-32 32L32 288c-17.7 0-32-14.3-32-32z"],
  folder: [512, "M64 448l384 0c35.3 0 64-28.7 64-64l0-240c0-35.3-28.7-64-64-64L298.7 80c-6.9 0-13.7-2.2-19.2-6.4L241.1 44.8C230 36.5 216.5 32 202.7 32L64 32C28.7 32 0 60.7 0 96L0 384c0 35.3 28.7 64 64 64z"],
  folderOpen: [576, "M56 225.6L32.4 296.2 32.4 96c0-35.3 28.7-64 64-64l138.7 0c13.8 0 27.3 4.5 38.4 12.8l38.4 28.8c5.5 4.2 12.3 6.4 19.2 6.4l117.3 0c35.3 0 64 28.7 64 64l0 16-365.4 0c-41.3 0-78 26.4-91.1 65.6zM477.8 448L99 448c-32.8 0-55.9-32.1-45.5-63.2l48-144C108 221.2 126.4 208 147 208l378.8 0c32.8 0 55.9 32.1 45.5 63.2l-48 144c-6.5 19.6-24.9 32.8-45.5 32.8z"],
  trash: [448, "M136.7 5.9C141.1-7.2 153.3-16 167.1-16l113.9 0c13.8 0 26 8.8 30.4 21.9L320 32 416 32c17.7 0 32 14.3 32 32s-14.3 32-32 32L32 96C14.3 96 0 81.7 0 64S14.3 32 32 32l96 0 8.7-26.1zM32 144l384 0 0 304c0 35.3-28.7 64-64 64L96 512c-35.3 0-64-28.7-64-64l0-304zm88 64c-13.3 0-24 10.7-24 24l0 192c0 13.3 10.7 24 24 24s24-10.7 24-24l0-192c0-13.3-10.7-24-24-24zm104 0c-13.3 0-24 10.7-24 24l0 192c0 13.3 10.7 24 24 24s24-10.7 24-24l0-192c0-13.3-10.7-24-24-24zm104 0c-13.3 0-24 10.7-24 24l0 192c0 13.3 10.7 24 24 24s24-10.7 24-24l0-192c0-13.3-10.7-24-24-24z"],
  done: [512, "M256 512a256 256 0 1 1 0-512 256 256 0 1 1 0 512zM374 145.7c-10.7-7.8-25.7-5.4-33.5 5.3L221.1 315.2 169 263.1c-9.4-9.4-24.6-9.4-33.9 0s-9.4 24.6 0 33.9l72 72c5 5 11.8 7.5 18.8 7s13.4-4.1 17.5-9.8L379.3 179.2c7.8-10.7 5.4-25.7-5.3-33.5z"],
  error: [512, "M256 512a256 256 0 1 1 0-512 256 256 0 1 1 0 512zm0-192a32 32 0 1 0 0 64 32 32 0 1 0 0-64zm0-192c-18.2 0-32.7 15.5-31.4 33.7l7.4 104c.9 12.6 11.4 22.3 23.9 22.3 12.6 0 23-9.7 23.9-22.3l7.4-104c1.3-18.2-13.1-33.7-31.4-33.7z"],
  spinner: [512, "M222.7 32.1c5 16.9-4.6 34.8-21.5 39.8-79.3 23.6-137.1 97.1-137.1 184.1 0 106 86 192 192 192s192-86 192-192c0-86.9-57.8-160.4-137.1-184.1-16.9-5-26.6-22.9-21.5-39.8s22.9-26.6 39.8-21.5C434.9 42.1 512 140 512 256 512 397.4 397.4 512 256 512S0 397.4 0 256c0-116 77.1-213.9 182.9-245.4 16.9-5 34.8 4.6 39.8 21.5z"],
  cancelled: [512, "M367.2 412.5L99.5 144.8c-22.4 31.4-35.5 69.8-35.5 111.2 0 106 86 192 192 192 41.5 0 79.9-13.1 111.2-35.5zm45.3-45.3c22.4-31.4 35.5-69.8 35.5-111.2 0-106-86-192-192-192-41.5 0-79.9 13.1-111.2 35.5L412.5 367.2zM0 256a256 256 0 1 1 512 0 256 256 0 1 1 -512 0z"],
  desktop: [512, "M64 32C28.7 32 0 60.7 0 96L0 352c0 35.3 28.7 64 64 64l144 0-16 48-72 0c-13.3 0-24 10.7-24 24s10.7 24 24 24l272 0c13.3 0 24-10.7 24-24s-10.7-24-24-24l-72 0-16-48 144 0c35.3 0 64-28.7 64-64l0-256c0-35.3-28.7-64-64-64L64 32zM96 96l320 0c17.7 0 32 14.3 32 32l0 160c0 17.7-14.3 32-32 32L96 320c-17.7 0-32-14.3-32-32l0-160c0-17.7 14.3-32 32-32z"],
  gamepad: [640, "M448 64c106 0 192 86 192 192S554 448 448 448l-256 0C86 448 0 362 0 256S86 64 192 64l256 0zM192 176c-13.3 0-24 10.7-24 24l0 32-32 0c-13.3 0-24 10.7-24 24s10.7 24 24 24l32 0 0 32c0 13.3 10.7 24 24 24s24-10.7 24-24l0-32 32 0c13.3 0 24-10.7 24-24s-10.7-24-24-24l-32 0 0-32c0-13.3-10.7-24-24-24zm240 96a32 32 0 1 0 0 64 32 32 0 1 0 0-64zm64-96a32 32 0 1 0 0 64 32 32 0 1 0 0-64z"],
  box: [640, "M560.3 237.2c10.4 11.8 28.3 14.4 41.8 5.5 14.7-9.8 18.7-29.7 8.9-44.4l-48-72c-2.8-4.2-6.6-7.7-11.1-10.2L351.4 4.7c-19.3-10.7-42.8-10.7-62.2 0L88.8 116c-5.4 3-9.7 7.4-12.6 12.8L27.7 218.7c-12.6 23.4-3.8 52.5 19.6 65.1l33 17.7 0 53.3c0 23 12.4 44.3 32.4 55.7l176 99.7c19.6 11.1 43.5 11.1 63.1 0l176-99.7c20.1-11.4 32.4-32.6 32.4-55.7l0-117.5zm-240-9.8L170.2 144 320.3 60.6 470.4 144 320.3 227.4zm-41.5 50.2l-21.3 46.2-165.8-88.8 25.4-47.2 161.7 89.8z"],
  shield: [512, "M256 0c4.6 0 9.2 1 13.4 2.9L457.8 82.8c22 9.3 38.4 31 38.3 57.2-.5 99.2-41.3 280.7-213.6 363.2-16.7 8-36.1 8-52.8 0-172.4-82.5-213.1-264-213.6-363.2-.1-26.2 16.3-47.9 38.3-57.2L242.7 2.9C246.9 1 251.4 0 256 0zm0 66.8l0 378.1c138-66.8 175.1-214.8 176-303.4l-176-74.6 0 0z"],
  broom: [576, "M566.6 54.6c12.5-12.5 12.5-32.8 0-45.3s-32.8-12.5-45.3 0l-192 192-34.7-34.7c-4.2-4.2-10-6.6-16-6.6-12.5 0-22.6 10.1-22.6 22.6l0 29.1 108.3 108.3 29.1 0c12.5 0 22.6-10.1 22.6-22.6 0-6-2.4-11.8-6.6-16l-34.7-34.7 192-192zM341.1 353.4L222.6 234.9c-42.7-3.7-85.2 11.7-115.8 42.3l-8 8c-22.3 22.3-34.8 52.5-34.8 84 0 6.8 7.1 11.2 13.2 8.2l51.1-25.5c5-2.5 9.5 4.1 5.4 7.9L7.3 473.4C2.7 477.6 0 483.6 0 489.9 0 502.1 9.9 512 22.1 512l173.3 0c38.8 0 75.9-15.4 103.4-42.8 30.6-30.6 45.9-73.1 42.3-115.8z"],
};
const SVG = "http://www.w3.org/2000/svg";
function icon(name, className = "icon") {
  const [width, d] = ICONS[name];
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", `0 0 ${width} 512`);
  svg.setAttribute("class", className);
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", d);
  svg.append(path);
  return svg;
}
// Значок меняется только при смене имени: перерисовка на каждое событие
// прогресса перезапускала бы вращение индикатора.
function setIcon(element, name, className) {
  if (element.dataset.current === name) return;
  element.dataset.current = name || "";
  element.replaceChildren(...(name ? [icon(name, className)] : []));
}
for (const element of document.querySelectorAll("[data-icon]")) element.append(icon(element.dataset.icon));
const strings = {
  ru: {
    title: "Установка HLSR",
    description: "Выбери, куда поставить HLSR, и нажми «Установить».",
    welcomeDescription: "Сейчас на этот компьютер установится HLSR, лаунчер для спидранов Half-Life.",
    welcomeBuilds: "Сборки для забегов", welcomeBuildsHint: "Half-Life WON, Half-Life 2 и Ghosting ставятся из лаунчера",
    welcomePlay: "Запуск и настройки", welcomePlayHint: "Конфиги, бинды и LiveSplit под каждую игру",
    welcomeUpdate: "Обновления", welcomeUpdateHint: "Лаунчер обновляется сам",
    defenderTitle: "Windows может ругаться",
    defenderDescription: "Windows Defender и SmartScreen не знают HLSR и могут предупредить о нём или удалить его файл. Если антивирус остановит установку, установщик подскажет, что делать.",
    calloutLead: "Предупреждение Windows не значит, что в HLSR вирус.",
    calloutTitle: "Если SmartScreen пишет «Система Windows защитила ваш компьютер»",
    calloutSteps: "Нажми «Подробнее», потом «Выполнить в любом случае».",
    calloutWhy: "Предупреждение безвредно. Оно появляется только потому, что у HLSR нет подписи кода, а подпись стоит около 300 € в год.",
    next: "Далее", step: (n, total) => `Шаг ${n} из ${total}`,
    antivirusTitle: "Антивирус остановил установку",
    antivirusDescription: "Антивирус удалил или заблокировал файл HLSR. С программами без подписи кода такое бывает.",
    antivirusHelp: (file, folder) => [
      `Файл: ${file}`,
      "1. Разреши файл в антивирусе. В Windows Defender: «Безопасность Windows» → «Защита от вирусов и угроз» → «Журнал защиты» → «Разрешить на устройстве».",
      `2. Или добавь папку ${folder} в исключения антивируса.`,
      "3. Закрой установщик и запусти его снова.",
      "Не помогло? Напиши нам в Discord HLSR, поможем.",
    ].join("\n"),
    discord: "Открыть Discord",
    location: "Каталог установки",
    desktop: "Ярлык на рабочем столе", desktopHint: "Ярлык в меню приложений создаётся всегда",
    install: "Установить", retry: "Повторить", launch: "Открыть HLSR", busy: "Установка…",
    update: "Обновить", updateTitle: "Обновление HLSR", updateBusy: "Обновление…",
    updateDescription: "Игры, сборки и настройки останутся на месте.",
    installingDescription: "Ставим HLSR, это недолго.",
    updatingDescription: "Обновляем HLSR. Игры, сборки и настройки останутся на месте.",
    version: (value) => `Версия ${value}`, installed: (value) => `Установлена ${value}`,
    verifying: "Проверяем и распаковываем", installing: "Создаём установку и ярлыки", updating: "Обновляем файлы лаунчера",
    doneTitle: "HLSR установлен", doneDescription: "Можно начинать. Лаунчер будет получать обновления автоматически. Скачанный установщик больше не нужен.",
    errorTitle: "Не удалось установить", updateErrorTitle: "Не удалось обновить",
    errorDescription: "Что-то помешало установке, подробности ниже.",
    noSpaceDescription: "На диске не хватает места. Освободи место и нажми «Повторить».",
    noAccessDescription: "Нет доступа к папке, или её файлы заняты. Закрой HLSR и всё, что открыто из этой папки, и нажми «Повторить».",
    systemDescription: "Система не дала записать файлы. Нажми «Повторить» или выбери другую папку.",
    errorCode: (code) => `Код ошибки для поддержки: ${code}`,
    units: ["КБ", "МБ", "ГБ"], decimal: ",",
    uninstall: "Удалить HLSR",
    uninstallTitle: "Удаление HLSR",
    uninstallDescription: "Будут удалены файлы лаунчера и его ярлыки.",
    uninstallingDescription: "Удаляем HLSR, это недолго.",
    gamesBlocked: {
      other: "Игры и настройки останутся: библиотекой пользуется другая установка HLSR.",
      test: "Тестовая сборка не удаляет игры и настройки.",
      unverified: "Игры и настройки останутся: не удалось проверить, что это та самая установка HLSR.",
    },
    games: "Также удалить игры", gamesHint: (size, where) => ["Вместе с сейвами, демками, настройками и входом", size, where].filter(Boolean).join(" · "),
    uninstalling: "Удаляем HLSR",
    uninstalledTitle: "HLSR удалён",
    uninstalledDescription: "Лаунчер удалён с этого компьютера. Скачанный установщик больше не нужен.",
    close: "Закрыть", back: "Назад",
    cancel: "Отмена",
    cancelling: "Отменяем установку…", cancelledTitle: "Установка отменена",
    cancelledDescription: "Можно выбрать другую папку и начать установку снова.",
    browse: "Выбрать папку", minimize: "Свернуть",
  },
  en: {
    title: "Install HLSR",
    description: "Choose where to install HLSR, then click Install.",
    welcomeDescription: "HLSR, a launcher for Half-Life speedruns, is about to be installed on this computer.",
    welcomeBuilds: "Speedrun builds", welcomeBuildsHint: "Install Half-Life WON, Half-Life 2 and Ghosting from the launcher",
    welcomePlay: "Play and set up", welcomePlayHint: "Configs, binds and LiveSplit for each game",
    welcomeUpdate: "Updates", welcomeUpdateHint: "The launcher updates itself",
    defenderTitle: "Windows may complain",
    defenderDescription: "Windows Defender and SmartScreen don't know HLSR yet and may warn you about it or delete one of its files. If an antivirus stops the install, the installer will tell you what to do.",
    calloutLead: "A Windows warning doesn't mean HLSR has a virus.",
    calloutTitle: "If SmartScreen says \"Windows protected your PC\"",
    calloutSteps: "Click \"More info\", then \"Run anyway\".",
    calloutWhy: "The warning is harmless. It appears only because HLSR has no code signature, and code signing costs about €300 a year.",
    next: "Next", step: (n, total) => `Step ${n} of ${total}`,
    antivirusTitle: "Antivirus stopped the installation",
    antivirusDescription: "Your antivirus deleted or blocked an HLSR file. This happens to programs without a code signature.",
    antivirusHelp: (file, folder) => [
      `File: ${file}`,
      "1. Allow the file in your antivirus. In Windows Defender: Windows Security → Virus & threat protection → Protection history → Allow on device.",
      `2. Or add the ${folder} folder to your antivirus exclusions.`,
      "3. Close the installer and run it again.",
      "Still not working? Message us on the HLSR Discord and we'll help.",
    ].join("\n"),
    discord: "Open Discord",
    location: "Install directory",
    desktop: "Desktop shortcut", desktopHint: "A menu shortcut is always created",
    install: "Install", retry: "Retry", launch: "Open HLSR", busy: "Installing…",
    update: "Update", updateTitle: "Update HLSR", updateBusy: "Updating…",
    updateDescription: "Games, builds and settings stay in place.",
    installingDescription: "Installing HLSR, this won't take long.",
    updatingDescription: "Updating HLSR. Games, builds and settings stay in place.",
    version: (value) => `Version ${value}`, installed: (value) => `Installed ${value}`,
    verifying: "Verifying and unpacking", installing: "Installing HLSR and shortcuts", updating: "Updating launcher files",
    doneTitle: "HLSR is installed", doneDescription: "Ready when you are. The launcher will update automatically. You can delete the downloaded installer.",
    errorTitle: "Installation failed", updateErrorTitle: "Update failed",
    errorDescription: "Something went wrong, see the details below.",
    noSpaceDescription: "The disk is out of space. Free some up and click Retry.",
    noAccessDescription: "HLSR can't write to this folder, or its files are in use. Close HLSR and anything opened from the folder, then click Retry.",
    systemDescription: "Couldn't write the files. Click Retry or choose another folder.",
    errorCode: (code) => `Error code for support: ${code}`,
    units: ["KB", "MB", "GB"], decimal: ".",
    uninstall: "Uninstall HLSR",
    uninstallTitle: "Uninstall HLSR",
    uninstallDescription: "Launcher files and shortcuts will be removed.",
    uninstallingDescription: "Uninstalling HLSR, this won't take long.",
    gamesBlocked: {
      other: "Games and settings stay: another HLSR installation uses this library.",
      test: "Test builds don't delete games and settings.",
      unverified: "Games and settings stay: couldn't verify this HLSR installation.",
    },
    games: "Also delete games", gamesHint: (size, where) => ["Saves, demos, settings and sign-in too", size, where].filter(Boolean).join(" · "),
    uninstalling: "Uninstalling HLSR",
    uninstalledTitle: "HLSR is uninstalled",
    uninstalledDescription: "The launcher is removed from this computer. You can delete the downloaded installer.",
    close: "Close", back: "Back",
    cancel: "Cancel",
    cancelling: "Cancelling installation…", cancelledTitle: "Installation cancelled",
    cancelledDescription: "You can choose another folder and start installation again.",
    browse: "Choose folder", minimize: "Minimize",
  },
};
// Размер на языке окна: «12,4 ГБ» / «12.4 GB».
function formatBytes(bytes, text) {
  const value = Number(bytes);
  if (!isFinite(value) || value <= 0) return "";
  const [size, unit] = value < 1048576 ? [String(Math.ceil(value / 1024)), 0]
    : value < 1073741824 ? [(value / 1048576).toFixed(0), 1] : [(value / 1073741824).toFixed(1), 2];
  return `${size.replace(".", text.decimal)}\u00a0${text.units[unit]}`;
}
// Ошибка ОС по коду (installer-window.js присылает код, сырой текст пишет в
// журнал): нет места, нет доступа или файлы заняты, остальное.
const ERROR_KINDS = { ENOSPC: "noSpace", EDQUOT: "noSpace", EACCES: "noAccess", EPERM: "noAccess", EBUSY: "noAccess", EROFS: "noAccess" };
function errorKind(code) {
  const value = String(code || "");
  return ERROR_KINDS[value] || (/^E[A-Z]+$/.test(value) || value === "UNKNOWN" ? "system" : null);
}
function chip(text, className = "chip") {
  const element = document.createElement("span");
  element.className = className;
  element.textContent = text;
  return element;
}
// Шаги перед формой новой установки: приветствие, в Windows ещё SmartScreen.
// Обновление, ошибка и готовая установка открываются сразу на своём экране.
let intro = null, introStep = 0, introBack = false;
// Папка для исключений антивируса — родитель каталога установки: рядом с
// ним собирается новая версия. Каталог в корне диска — он сам.
function exclusionFolder(dir) {
  const parent = String(dir || "").replace(/[\\/]+[^\\/]*$/, "");
  return !parent || /^[A-Za-z]:$/.test(parent) ? dir : parent;
}
// Чипы под заголовком: версия, а при обновлении — «было → станет» значком.
function renderMeta(text, busy) {
  const items = [];
  if (state.phase === "idle" && intro.length) items.push(chip(text.step(introStep + 1, intro.length + 1)));
  if ((state.phase === "idle" || busy) && state.update && state.installedVersion && state.version) {
    const pair = document.createElement("span");
    pair.className = "chip chip--update";
    pair.append(chip(state.installedVersion, "chip__from"), icon("arrowRight", "icon chip__arrow"), chip(state.version, "chip__to"));
    items.push(pair);
  } else if (state.phase === "done" && state.installedVersion) items.push(chip(text.installed(state.installedVersion)));
  else if (state.version && !["uninstall-confirm", "uninstalling", "uninstalled"].includes(state.phase))
    items.push(chip(text.version(state.version)));
  $("meta").replaceChildren(...items);
  $("meta").hidden = !items.length;
}
// Значок в квадрате слева от заголовка показывает, что происходит.
function badgeFor(busy, step, antivirus) {
  if (step === "defender") return ["shield", "caution"];
  if (antivirus) return ["shield", "danger"];
  if (state.phase === "done") return ["done", "ok"];
  if (state.phase === "error") return ["error", "danger"];
  if (state.phase === "cancelled") return ["cancelled", "muted"];
  if (state.phase === "uninstalled") return ["broom", "muted"];
  if (state.phase === "uninstall-confirm" || state.phase === "uninstalling") return ["trash", "danger"];
  if (busy) return ["box", ""];
  return [state.update ? "update" : "download", ""];
}
let state = { phase: "idle" };
// Состояние до подтверждения удаления: «Отмена» возвращает к нему.
let beforeUninstall = null;
function render(next) {
  state = next;
  if (!intro) intro = state.phase === "idle" && !state.update ? (state.platform === "win32" ? ["welcome", "defender"] : ["welcome"]) : [];
  const step = state.phase === "idle" && introStep < intro.length ? intro[introStep] : null;
  const antivirus = state.phase === "error" && state.errorCode === "ANTIVIRUS";
  const text = strings[state.language] || strings.ru;
  document.documentElement.lang = state.language || "ru";
  const busy = !["idle", "cancelled", "error", "done", "uninstall-confirm", "uninstalled"].includes(state.phase);
  const uninstalling = state.phase === "uninstall-confirm" || state.phase === "uninstalling" || state.phase === "uninstalled";
  $("welcome").hidden = step !== "welcome";
  $("defender").hidden = step !== "defender";
  if (step === "welcome") {
    $("title").textContent = text.title;
    $("description").textContent = text.welcomeDescription;
    for (const key of ["Builds", "Play", "Update"]) {
      $(`welcome-${key.toLowerCase()}`).textContent = text[`welcome${key}`];
      $(`welcome-${key.toLowerCase()}-hint`).textContent = text[`welcome${key}Hint`];
    }
  } else if (step === "defender") {
    $("title").textContent = text.defenderTitle;
    $("description").textContent = text.defenderDescription;
    $("callout-lead").textContent = text.calloutLead;
    $("callout-title").textContent = text.calloutTitle;
    $("callout-steps").textContent = text.calloutSteps;
    $("callout-why").textContent = text.calloutWhy;
  } else if (antivirus) {
    $("title").textContent = text.antivirusTitle;
    $("description").textContent = text.antivirusDescription;
  } else if (state.phase === "uninstall-confirm" || state.phase === "uninstalling") {
    $("title").textContent = text.uninstallTitle;
    $("description").textContent = state.phase === "uninstalling" ? text.uninstallingDescription : text.uninstallDescription;
  } else if (busy) {
    // Установка и обновление идут: совет с формы уже не к месту.
    $("title").textContent = state.update ? text.updateTitle : text.title;
    $("description").textContent = state.update ? text.updatingDescription : text.installingDescription;
  } else if (state.phase === "error") {
    $("title").textContent = state.update ? text.updateErrorTitle : text.errorTitle;
    $("description").textContent = text[`${errorKind(state.errorCode)}Description`] || text.errorDescription;
  } else if (state.phase === "uninstalled") {
    $("title").textContent = text.uninstalledTitle;
    $("description").textContent = text.uninstalledDescription;
  } else if (state.phase === "cancelled") {
    $("title").textContent = text.cancelledTitle;
    $("description").textContent = text.cancelledDescription;
  } else if (state.phase === "idle" && state.update) {
    $("title").textContent = text.updateTitle;
    $("description").textContent = text.updateDescription;
  } else {
    $("title").textContent = state.phase === "done" ? text.doneTitle : text.title;
    $("description").textContent = state.phase === "done" ? text.doneDescription : text.description;
  }
  renderMeta(text, busy && !uninstalling);
  const [badge, tone] = badgeFor(busy, step, antivirus);
  setIcon($("badge"), badge);
  $("badge").dataset.tone = tone;
  // После удаления каталога больше нет: путь к нему только сбивает с толку.
  // В ошибке антивируса папку называет совет, место нужно ему.
  $("destination").hidden = state.phase === "uninstalled" || !!step || antivirus;
  $("directory").value = state.directory || "";
  $("directory").title = state.directory || "";
  $("browse").disabled = busy || !state.canChooseDirectory || uninstalling;
  $("browse").hidden = !state.canChooseDirectory;
  $("browse").title = text.browse;
  $("browse").setAttribute("aria-label", text.browse);
  $("minimize").setAttribute("aria-label", text.minimize);
  $("minimize").title = text.minimize;
  $("close").setAttribute("aria-label", text.close);
  $("close").title = text.close;
  $("location-label").textContent = text.location;
  $("desktop-label").textContent = text.desktop;
  $("desktop-hint").textContent = text.desktopHint;
  $("desktop-option").hidden = busy || state.phase === "done" || uninstalling || !!step || antivirus;
  const plan = state.uninstallPlan;
  const showGames = state.phase === "uninstall-confirm" && plan && plan.libraryExists && plan.libraryBytes > 0;
  $("games-option").hidden = !showGames;
  if (showGames) {
    $("games-label").textContent = text.games;
    // Все папки, которые очистит удаление (в Windows ещё и папка HLSR вокруг программы).
    $("games-hint").textContent = text.gamesHint(formatBytes(plan.libraryBytes, text), (plan.libraryRoots || [plan.libraryRoot]).join(", "));
    $("games-hint").title = $("games-hint").textContent;
  }
  // Удалить можно и из ошибки: встроенный архив для удаления не нужен.
  const canUninstall = (state.phase === "done" || state.phase === "error") && state.canUninstall === true;
  $("uninstall").hidden = !canUninstall;
  $("uninstall-label").textContent = text.uninstall;
  $("uninstall").disabled = busy;
  $("discord").hidden = !antivirus;
  $("discord-label").textContent = text.discord;
  $("activity").hidden = !(busy || state.phase === "uninstalling");
  $("step").textContent = text[state.update && state.phase === "installing" ? "updating" : state.phase] || "";
  document.body.dataset.phase = step || state.phase;
  $("action").disabled = busy;
  // Надпись и значок основной кнопки по фазе. Пока идёт работа, значок —
  // крутящееся кольцо, как у загрузок в лаунчере.
  let action = state.update ? [text.update, "update"] : [text.install, "download"];
  if (step) action = [text.next, "arrowRight"];
  else if (busy) action = [state.phase === "uninstalling" ? text.uninstalling : state.update ? text.updateBusy : text.busy, "spinner"];
  else if (state.phase === "done") action = [text.launch, "open"];
  else if (state.phase === "error") action = [text.retry, "retry"];
  else if (state.phase === "uninstall-confirm") action = [text.uninstall, "trash"];
  else if (state.phase === "uninstalled") action = [text.close, "cancel"];
  $("action-label").textContent = action[0];
  setIcon($("action-icon"), action[1], action[1] === "spinner" ? "icon spin" : "icon");
  introBack = state.phase === "idle" && !state.cancellable && introStep > 0 && intro.length > 0;
  const dismiss = state.phase === "uninstalled" ? (state.canGoBack === false ? null : [text.back, "back"])
    : state.cancellable || state.phase === "cancelling" || state.phase === "uninstall-confirm" ? [text.cancel, "cancel"]
    : introBack ? [text.back, "back"] : null;
  $("dismiss").hidden = !dismiss;
  if (dismiss) {
    $("dismiss-label").textContent = dismiss[0];
    setIcon($("dismiss-icon"), dismiss[1]);
  }
  $("dismiss").disabled = state.phase === "cancelling";
  // Процент присылает только установка в Windows (копирование файлов);
  // без него полоса бегущая.
  const progress = busy && typeof state.progress === "number" ? state.progress : null;
  if (progress === null) $("progress").removeAttribute("value");
  else $("progress").value = progress;
  $("progress-text").textContent = progress === null ? "" : `${progress}%`;
  // Переключателя игр нет из-за защиты (другая установка, тестовая сборка, проверка не прошла): почему.
  const blocked = state.phase === "uninstall-confirm" && plan && plan.libraryBlocked
    ? text.gamesBlocked[plan.libraryBlocked] || text.gamesBlocked.other : null;
  const warning = state.warning || blocked;
  $("error").hidden = !state.error && !warning;
  $("error").dataset.tone = state.error && !antivirus ? "danger" : "warning";
  // Сырой текст ошибки ОС («ENOSPC: no space left…») окну не показываем: он в журнале.
  $("error-text").textContent = antivirus ? text.antivirusHelp(state.errorFile || "", exclusionFolder(state.directory))
    : state.error && errorKind(state.errorCode) ? text.errorCode(state.errorCode)
    : state.error || warning || "";
}
$("action").addEventListener("click", async () => {
  if (state.phase === "idle" && introStep < intro.length) {
    introStep += 1;
    render(state);
    return;
  }
  $("action").disabled = true;
  try {
    if (state.phase === "done") await window.installer.launch();
    else if (state.phase === "uninstall-confirm") await window.installer.uninstall(!$("games-option").hidden && $("games").checked);
    else if (state.phase === "uninstalled") window.installer.windowCommand("close");
    else await window.installer.install($("desktop").checked);
  } catch (error) { render({ ...state, phase: "error", error: error.message, errorCode: null, errorFile: null }); }
  finally { render(state); }
});
$("uninstall").addEventListener("click", async () => {
  $("uninstall").disabled = true;
  try {
    const plan = await window.installer.uninstallPlan();
    if (!plan) return;
    // Решение владельца: по умолчанию удаление полное, папка HLSR уходит целиком.
    $("games").checked = true;
    beforeUninstall = state;
    render({ ...state, phase: "uninstall-confirm", error: null, warning: null, uninstallPlan: plan });
  } catch (error) { render({ ...state, phase: "error", error: error.message, errorCode: null, errorFile: null }); }
  finally { render(state); }
});
$("dismiss").addEventListener("click", () => {
  if (introBack) {
    introStep -= 1;
    render(state);
  } else if (state.phase === "uninstalled") window.installer.back();
  else if (state.phase === "uninstall-confirm" && beforeUninstall) render(beforeUninstall);
  else window.installer.cancel();
});
$("discord").addEventListener("click", () => window.installer.openDiscord());
$("browse").addEventListener("click", async () => {
  $("browse").disabled = true;
  try { await window.installer.selectDirectory(); }
  catch (error) { render({ ...state, error: error.message, errorCode: null, errorFile: null }); }
  finally { render(state); }
});
window.installer.onState(render);
window.installer.state().then(render);

$("minimize").addEventListener("click", () => window.installer.windowCommand("minimize"));
$("close").addEventListener("click", () => window.installer.windowCommand("close"));
