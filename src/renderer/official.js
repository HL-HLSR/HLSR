// Официальная ли это сборка (src/main/security/official.js). В самосборке «Главная», вход
// через Steam, профили, друзья, спонсорство и Telegram заменены заглушкой со ссылкой на
// страницу выпусков.
export const OFFICIAL = window.hlsrElectron.ipcRenderer.sendSync("app-official") === true;
