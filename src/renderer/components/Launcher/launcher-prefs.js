
const STORAGE_KEY = "hlsr-launcher-prefs";
const { normalizeSpeedLimit } = require("../../../shared/download-settings");

export const DEFAULT_INSTALL_PATH = window.hlsrElectron.defaultLibraryPath;

export const ACCENT = {
  accent: "#00ABFF",
  rgb: "0, 171, 255",
  hover: "#2FBAFF",
  pressed: "#0090DA",
  soft: "rgba(0, 171, 255, 0.16)",
  border: "rgba(0, 171, 255, 0.55)",
  focus: "#4FC3FF",
  contrastText: "#FFFFFF",
};

const DEFAULTS = {
  speedLimit: 0,
  animations: true,
  hideWhilePlaying: true,
  // Кнопка «Полный экран» в шапке (WindowControls.vue); «Настройки» → «Интерфейс».
  fullscreenButton: true,
  hideSteamId: false,
  timerOffered: false,
  // Сплиты LiveSplit по играм ({ gameId: id набора }), выбираются в окне
  // запуска LiveSplit (LiveSplitLaunchModal.vue).
  toolSplits: {},
  // Игра, для которой LiveSplit запускали в прошлый раз (id из games.config).
  liveSplitGame: "",
  coloredBoot: true,
  // Тема из launcher-themes.js; выбирается в «Настройки» → «Интерфейс».
  launcherTheme: "hlsr",
  // Акцент темы «Бумага» (PAPER_ACCENTS в launcher-themes.js): clay, rose, amber, sea.
  paperAccent: "clay",
  // Колонка значков выдвинута: видны названия игр и онлайн.
  railExpanded: false,
  // Окно «HLSR — Новая эра!» уже показано (WelcomeModal.vue).
  welcomeSeen: false,
  // Обучение по странице «Персонализация» (ConfigTour.vue) пройдено или пропущено.
  configTourSeen: false,
  // Категория проверки конфига по правилам speedrun.com для игры: { "hl1-won": "scripted" }.
  // Пишет только setRuleCategory; save() чужим снимком prefs её не затирает.
  ruleCategory: {},
  // Клавиатура на вкладке «Клавиатура»: false — только основной блок, true — вся.
  keyboardFull: false,
  // «Версия сборки» HL2 Ghosting: "HL1 MOVEMENT" или эпизод (launcher-data.js, GHOSTING_EPISODES).
  ghostingBuild: "HL1 MOVEMENT",
};

const REMOVED_KEYS = [
  "autoUpdate",
  "uiScale",
  "telemetry",
  "colorTheme",
  "installPath",
  "concurrent",
  "accent",
  "autostart",
  "tray",
  "onClose",
];

function load() {
  let saved;

  try {
    saved = JSON.parse(localStorage.getItem(STORAGE_KEY)) || {};
  } catch (e) {
    saved = {};
  }

  if (typeof saved !== "object" || Array.isArray(saved)) saved = {};

  for (const key of REMOVED_KEYS) delete saved[key];
  saved.speedLimit = normalizeSpeedLimit(saved.speedLimit);

  return Object.assign({}, DEFAULTS, saved);
}

function write(prefs) {
  const clean = Object.assign({}, prefs);
  for (const key of REMOVED_KEYS) delete clean[key];
  clean.speedLimit = normalizeSpeedLimit(clean.speedLimit);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(clean));
}

function save(prefs) {
  const kept = load();
  write(Object.assign({}, prefs, { ruleCategory: kept.ruleCategory, keyboardFull: kept.keyboardFull }));
}

const ruleCategories = (value) => (value && typeof value === "object" && !Array.isArray(value) ? value : {});

// "scripted" или "scriptless" (по умолчанию) для игры.
function ruleCategoryOf(gameId = "hl1-won") {
  return ruleCategories(load().ruleCategory)[gameId] === "scripted" ? "scripted" : "scriptless";
}

function setRuleCategory(gameId, category) {
  const prefs = load();
  prefs.ruleCategory = Object.assign({}, ruleCategories(prefs.ruleCategory), { [gameId]: category === "scripted" ? "scripted" : "scriptless" });
  write(prefs);
}

function setKeyboardFull(full) {
  write(Object.assign(load(), { keyboardFull: !!full }));
}

function defaults() {
  return Object.assign({}, DEFAULTS);
}

export default { load, save, defaults, ruleCategoryOf, setRuleCategory, setKeyboardFull };
