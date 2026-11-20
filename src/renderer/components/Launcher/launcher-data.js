export const GHOSTING_GAME_ID = "218";

// Игры на GoldSrc: переключатели запуска Source (-novid, -dx9 и т. п.) им не нужны.
export const GOLDSRC_GAME_IDS = ["70"];

// Под чьей лицензией Steam идёт сборка: название для диалогов и ссылка на магазин.
const LICENSE_GAMES = {
  "70": { title: "Half-Life", storeLink: "steamHalfLife" },
};
const DEFAULT_LICENSE_GAME = { title: "Half-Life 2", storeLink: "steamHalfLife2" };

// Эпизоды Ghosting в «Версии сборки» — дополнения её установки, а не сборки main:
// выбор хранит лаунчер (prefs.ghostingBuild). На Linux main эпизоды не запускает.
const GHOSTING_EPISODES = window.hlsrElectron.platform === "linux" ? {} : { "Episode One": "ep1", "Episode Two": "ep2" };

export function licenseGameOf(gameId) {
  return LICENSE_GAMES[gameId] || DEFAULT_LICENSE_GAME;
}

export const DELIVERY_IDS = {
  "70": "hl1-won",
  "220": "hl2-4260",
  [GHOSTING_GAME_ID]: "ghosting-renewed",
};



// HL2 и эпизоды: арт Dhabih Eng (обои к 20-летию HL2, 3840x2160), разобранный
// на два слоя для параллакса. Фон — дымка без персонажей (место под ними
// залито по краям), верх затемнён: белая дымка засвечивала верхнюю панель
// лаунчера. Гордон и Аликс — вырезка тем же кадром 16:9 в рост окна:
// сдвинута чуть вправо, чтобы гравипушка не наезжала на статистику, и
// опущена, чтобы головы не упирались в верхнюю панель.
const HL2_ART = {
  hero: require("@/assets/launcher/hero-hl2.jpg"),
  heroFront: require("@/assets/launcher/hl2-front.webp"),
  heroFrontLayout: {
    "--front-size": "auto 100cqh",
    "--front-pos": "calc(var(--front-bleed-x) + 106cqw - 178cqh) 9cqh",
    "--front-origin-x": "70cqw",
    "--front-origin-y": "70cqh",
  },
};

const gameArt = {
  "70": {
    // Фон — лаборатория аномальных материалов (c1a0e), снята в самой игре
    // без людей; передний план — промо-арт Гордона 1998 года в полный рост
    // (Combine OverWiki, уже с прозрачным фоном). Как и в Ghosting, фигура
    // крупнее окна и стоит в правой части экрана, а низ окна срезает её
    // чуть ниже пояса. Позиция в долях окна: рамка слоя шире окна, и
    // привязка к её краю увела бы Гордона за край экрана.
    hero: require("@/assets/launcher/hl1-bg.jpg"),
    heroFront: require("@/assets/launcher/hl1-front.png"),
    heroFrontLayout: {
      "--front-size": "auto 170cqh",
      "--front-pos": "calc(50% + 22cqw) 10cqh",
      "--front-origin-x": "72cqw",
      "--front-origin-y": "70cqh",
    },
    logo: require("@/assets/launcher/logos/half-life.svg"),
    accent: "#ffa41c",
    accentSoft: "rgba(255, 164, 28, 0.16)",
  },
  "50": {
    hero: require("@/assets/screenshots/opposing-force-background.jpg"),
    logo: require("@/assets/launcher/logos/opposing-force.svg"),
    accent: "#7fd334",
    accentSoft: "rgba(127, 211, 52, 0.16)",
  },
  "130": {
    hero: require("@/assets/screenshots/blue-shift-background.jpg"),
    logo: require("@/assets/launcher/logos/blue-shift.svg"),
    accent: "#3fa7ff",
    accentSoft: "rgba(63, 167, 255, 0.16)",
  },
  "220": {
    ...HL2_ART,
    logo: require("@/assets/launcher/logos/half-life-2.svg"),
    accent: "#ff7a1c",
    accentSoft: "rgba(255, 122, 28, 0.16)",
  },
  "380": {
    ...HL2_ART,
    logo: require("@/assets/launcher/logos/episode-one.svg"),
    accent: "#ffb01c",
    accentSoft: "rgba(255, 176, 28, 0.16)",
  },
  "420": {
    ...HL2_ART,
    logo: require("@/assets/launcher/logos/episode-two.svg"),
    accent: "#ffd21c",
    accentSoft: "rgba(255, 210, 28, 0.16)",
  },
  [GHOSTING_GAME_ID]: {
    hero: require("@/assets/launcher/hl2-ghosting-bg.png"),
    // Фон карточки, когда в «Версии сборки» выбран эпизод (LauncherShell.backgroundSrc).
    episodesHero: require("@/assets/launcher/hl2-episodes-bg.jpg"),
    heroFront: require("@/assets/launcher/hl2-ghosting-front.webp"),
    logo: require("@/assets/launcher/logos/ghosting.svg"),
    accent: "#ff7a1c",
    accentSoft: "rgba(255, 122, 28, 0.16)",
  },
};

const demoState = {
  "70": {
    lastLaunchDays: null,
    buildUpdated: null,
    builds: ["WON 2005"],
    download: null,
  },
  "50": {
    lastLaunchDays: 6,
    buildUpdated: "02.02.2025 19:05",
    builds: ["GoldSrc Package", "WON", "Steam Legacy"],
    download: { done: 513, total: 513 },
  },
  "130": {
    lastLaunchDays: null,
    buildUpdated: "17.01.2025 12:44",
    builds: ["GoldSrc Package", "Steam Legacy"],
    download: { done: 285, total: 285 },
  },
  "220": {
    lastLaunchDays: null,
    buildUpdated: null,
    builds: ["4260", "2707", "5288"],
    download: null,
  },
  "218": {
    lastLaunchDays: null,
    buildUpdated: null,
    builds: ["HL1 MOVEMENT", ...Object.keys(GHOSTING_EPISODES)],
    episodes: GHOSTING_EPISODES,
    download: null,
  },
};

const shortNames = {
  "70": "Half-Life",
  "50": "Opposing Force",
  "130": "Blue Shift",
  "220": "Half-Life 2",
  "218": "HL2 Ghosting",
  "380": "HL2: Episode One",
  "420": "HL2: Episode Two",
};

// Логотипы инструментов в колонке значков — в том же кольце, что у игр.
export const toolArt = {
  livesplit: { logo: require("@/assets/launcher/logos/livesplit.svg") },
};

const heroSubtitles = {
  "218": "Ghosting Mod: Renewed",
};

export default { gameArt, demoState, shortNames, heroSubtitles, DELIVERY_IDS };
