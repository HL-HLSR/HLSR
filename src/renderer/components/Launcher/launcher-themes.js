// Темы лаунчера. Тема — набор CSS-переменных, которые оболочка ставит на
// .launcher-root поверх значений из launcher.css: акцент, тон кнопки
// «Играть», установки и загрузки, поверхности окон и затемнение подложки. Фон игры
// тема не трогает — он рисуется поверх подложки.
//
// swatch — три цвета для карточки темы в «Настройки» → «Интерфейс»:
// фон окна, поверхность и акцент.

function accent(hex, rgb, hover, pressed, contrastText) {
  return {
    "--accent": hex,
    "--accent-rgb": rgb,
    "--accent-hover": hover,
    "--accent-pressed": pressed,
    "--accent-soft": `rgba(${rgb}, 0.16)`,
    "--accent-border": `rgba(${rgb}, 0.55)`,
    "--accent-focus": hover,
    "--accent-contrast-text": contrastText || "#FFFFFF",
    "--accent-color": hex,
    "--btn-ready-bg": hex,
    "--btn-ready-hover": hover,
    "--btn-ready-pressed": pressed,
    "--btn-ready-glow": `rgba(${rgb}, 0.32)`,
    "--btn-ready-text": contrastText || "#FFFFFF",
    // Установка и загрузки сохраняют оттенок темы, но остаются темнее запуска.
    "--btn-progress-bg": `color-mix(in srgb, ${hex} 75%, #000)`,
    "--btn-progress-hover": `color-mix(in srgb, ${hover} 75%, #000)`,
    "--btn-progress-pressed": `color-mix(in srgb, ${pressed} 75%, #000)`,
    "--btn-progress-glow": `rgba(${rgb}, 0.24)`,
    "--btn-progress-text": contrastText || "#FFFFFF",
  };
}

function surfaces(bg, raised, modal, nav, scrimTop, scrimBottom) {
  return {
    "--l-bg": bg,
    "--l-bg-raised": raised,
    "--l-surface-solid": raised,
    "--l-modal-bg": modal,
    "--l-modal-nav": nav,
    "--l-modal-bar": nav,
    "--l-app-scrim": `linear-gradient(180deg, ${scrimTop} 0%, ${scrimBottom} 100%)`,
  };
}

// Тона, которые в теме HLSR синие (launcher.css): каналы затемнения кадра,
// полупрозрачных панелей, карточек и рамок, а также приглушённый текст и
// значки колонки. Без них в любой теме оставались синие рамки, колонка
// значков и нижняя панель.
function tones(shade, deep, panel, tint, dim, faint, railGame, railIcon) {
  return {
    "--l-shade-rgb": shade,
    "--l-deep-rgb": deep,
    "--l-panel-rgb": panel,
    "--l-tint-rgb": tint,
    "--l-text-dim": dim,
    "--l-text-faint": faint,
    "--l-rail-game": railGame,
    "--l-rail-icon": railIcon,
  };
}

// Варианты акцента темы «Бумага» — маленький выбор под карточками тем
// (launcher-prefs: paperAccent). Первый — по умолчанию.
export const PAPER_ACCENTS = [
  { id: "clay", title: "#L_THEME_PAPER_CLAY", color: "#D5876D", vars: accent("#D5876D", "213, 135, 109", "#EFA38B", "#BF725A", "#231313") },
  { id: "rose", title: "#L_THEME_PAPER_ROSE", color: "#D78AA8", vars: accent("#D78AA8", "215, 138, 168", "#EBB0CA", "#C07493", "#24131D") },
  { id: "amber", title: "#L_THEME_PAPER_AMBER", color: "#D5AA6B", vars: accent("#D5AA6B", "213, 170, 107", "#EBC38B", "#BE9457", "#251B0E") },
  { id: "sea", title: "#L_THEME_PAPER_SEA", color: "#80B2BA", vars: accent("#80B2BA", "128, 178, 186", "#A6D2D5", "#6C9DA5", "#102024") },
];

export const DEFAULT_THEME = "hlsr";

export const THEMES = [
  {
    id: "hlsr",
    title: "#L_THEME_HLSR",
    hint: "#L_THEME_HLSR_HINT",
    swatch: ["#0b0e13", "#12161d", "#00ABFF"],
    // Значения launcher.css и ACCENT: тема по умолчанию ничего не меняет.
    vars: {},
  },
  {
    id: "lambda",
    title: "#L_THEME_LAMBDA",
    hint: "#L_THEME_LAMBDA_HINT",
    swatch: ["#100b08", "#1a1310", "#FF7A1C"],
    vars: Object.assign(
      accent("#FF7A1C", "255, 122, 28", "#FF9445", "#E0650E"),
      surfaces("#100b08", "#17110d", "#1a1310", "#140f0b", "rgba(20, 10, 4, 0.5)", "rgba(14, 7, 3, 0.66)"),
      tones("14, 9, 6", "24, 15, 9", "38, 24, 15", "255, 176, 120", "#c4b0a0", "#87725f", "#c6b8ab", "#9e8877")
    ),
  },
  {
    id: "resonance",
    title: "#L_THEME_RESONANCE",
    hint: "#L_THEME_RESONANCE_HINT",
    swatch: ["#08100b", "#0f1912", "#3EE07A"],
    vars: Object.assign(
      accent("#2FCF6C", "47, 207, 108", "#4FE086", "#22B058", "#04170B"),
      surfaces("#08100b", "#0d1510", "#0f1912", "#0b130e", "rgba(3, 14, 7, 0.5)", "rgba(2, 10, 5, 0.66)"),
      tones("5, 13, 8", "8, 21, 13", "14, 34, 21", "120, 235, 160", "#a4c3ad", "#67846f", "#b1c3b6", "#7c9885")
    ),
  },
  {
    id: "combine",
    title: "#L_THEME_COMBINE",
    hint: "#L_THEME_COMBINE_HINT",
    swatch: ["#0a1014", "#111a20", "#5CD6E6"],
    vars: Object.assign(
      accent("#3EC4D6", "62, 196, 214", "#62D4E3", "#2BA7B8", "#03181C"),
      surfaces("#0a1014", "#0f171c", "#111a20", "#0c1418", "rgba(5, 13, 18, 0.52)", "rgba(3, 9, 13, 0.68)"),
      tones("5, 12, 16", "8, 20, 26", "13, 31, 39", "110, 220, 235", "#a2bfc5", "#68868e", "#acbcc0", "#7b949a")
    ),
  },
  {
    id: "xen",
    title: "#L_THEME_XEN",
    hint: "#L_THEME_XEN_HINT",
    swatch: ["#0e0a16", "#171021", "#B07CFF"],
    vars: Object.assign(
      accent("#A06BFF", "160, 107, 255", "#B489FF", "#8752E6"),
      surfaces("#0e0a16", "#140e1e", "#171021", "#110c1a", "rgba(16, 6, 28, 0.5)", "rgba(10, 4, 20, 0.66)"),
      tones("11, 7, 18", "17, 11, 28", "28, 17, 45", "195, 160, 255", "#b6aacb", "#7b6e93", "#bbb3c9", "#8e81a7")
    ),
  },
  {
    id: "steam2003",
    title: "#L_THEME_STEAM",
    hint: "#L_THEME_STEAM_HINT",
    swatch: ["#3e4637", "#4c5844", "#C4B550"],
    vars: Object.assign(
      accent("#C4B550", "196, 181, 80", "#D6C863", "#A99A3E", "#1D2116"),
      surfaces("#2a3025", "#353d2f", "#3e4637", "#353d2f", "rgba(40, 46, 34, 0.62)", "rgba(30, 35, 26, 0.76)"),
      tones("30, 35, 26", "42, 48, 37", "62, 70, 55", "196, 205, 180", "#a8b29c", "#7d8873", "#c9cfbf", "#a3ad97"),
      {
        "--l-border": "rgba(255, 255, 255, 0.1)",
        "--l-modal-border": "rgba(136, 145, 128, 0.45)",
        "--l-text-dim": "#a8b29c",
        "--l-text-faint": "#7d8873",
      }
    ),
  },
  {
    id: "midnight",
    title: "#L_THEME_MIDNIGHT",
    hint: "#L_THEME_MIDNIGHT_HINT",
    swatch: ["#000000", "#0a0a0b", "#E8ECF1"],
    vars: Object.assign(
      accent("#E8ECF1", "232, 236, 241", "#FFFFFF", "#C9CED6", "#0A0B0D"),
      surfaces("#000000", "#070708", "#0a0a0b", "#050506", "rgba(0, 0, 0, 0.62)", "rgba(0, 0, 0, 0.78)"),
      tones("0, 0, 0", "8, 8, 9", "18, 18, 20", "255, 255, 255", "#a7aaaf", "#6c6f75", "#b5b8bc", "#85888d"),
      {
        // Акцент почти белый: бегунок включённого переключателя тёмный,
        // иначе он сливается с дорожкой.
        "--l-switch-thumb-on": "#0A0B0D",
      }
    ),
  },
  {
    // Жидкое стекло iOS: поверхности полупрозрачные, а размытие кадра под
    // ними, блик по краю и тень дописывает launcher.css по
    // [data-theme="liquid-glass"] — переменными это не выразить.
    id: "liquid-glass",
    title: "#L_THEME_GLASS",
    hint: "#L_THEME_GLASS_HINT",
    swatch: ["#1b2330", "rgba(255, 255, 255, 0.18)", "#0A84FF"],
    vars: Object.assign(
      accent("#0A84FF", "10, 132, 255", "#409CFF", "#0071E3"),
      surfaces("#0d1118", "rgba(255, 255, 255, 0.07)", "rgba(24, 28, 36, 0.5)", "rgba(255, 255, 255, 0.04)",
        "rgba(4, 8, 14, 0.2)", "rgba(4, 8, 14, 0.38)"),
      tones("8, 12, 18", "16, 20, 28", "28, 32, 40", "255, 255, 255", "#c3c9d2", "#8a919c", "#d5dae1", "#a9b0ba"),
      {
        "--l-surface": "rgba(255, 255, 255, 0.1)",
        "--l-border": "rgba(255, 255, 255, 0.12)",
        "--l-border-strong": "rgba(255, 255, 255, 0.24)",
        "--l-modal-border": "rgba(255, 255, 255, 0.16)",
        "--l-modal-scrim": "rgba(2, 4, 8, 0.5)",
        "--l-radius": "18px",
        "--l-radius-sm": "12px",
        // На стекле поверх кадра цветам уровней нужен запас яркости.
        "--l-level-error": "#ff8a94",
        "--l-level-warning": "#ffdc6b",
        "--l-level-ok": "#79e384",
      }
    ),
  },
  {
    // Тёмная тетрадь: тёплая коричнево-бордовая палитра и приглушённый
    // акцент (accents). Формы — как у остальных тем; launcher.css по
    // [data-theme="paper"] дописывает только цвета, которые там заданы не
    // переменными, и тёплый оттенок кадра игры.
    id: "paper",
    title: "#L_THEME_PAPER",
    hint: "#L_THEME_PAPER_HINT",
    swatch: ["#1e1417", "#251a1c", "#D5876D"],
    accents: PAPER_ACCENTS,
    vars: Object.assign(
      // Подложка плотнее, чем у других тем: сквозь неё не проступает
      // голубой фирменный градиент.
      surfaces("#1e1417", "#251a1c", "#1e1417", "#170e10", "rgba(30, 20, 23, 0.84)", "rgba(23, 14, 16, 0.92)"),
      // Затемнение кадра (shade) — той же глубины, что у «Лямбды» и HLSR,
      // только в тоне палитры: светлее оно поднимало тени картинки в
      // розовато-серую дымку.
      tones("16, 10, 12", "23, 14, 16", "37, 26, 28", "224, 173, 157", "#bcaaa5", "#8f7c79", "#bcaaa5", "#8f7c79"),
      {
        "--l-text": "#eee3df",
        "--l-border": "#402a30",
        "--l-border-strong": "#5c3f46",
        "--l-modal-border": "#402a30",
        "--l-modal-scrim": "rgba(12, 7, 8, 0.6)",
        "--btn-repair-bg": "#c99a5c",
        "--btn-repair-hover": "#d8ab6e",
        "--btn-repair-pressed": "#b3874d",
        "--btn-repair-glow": "rgba(201, 154, 92, 0.3)",
        // Белый текст на кнопке удаления — контраст не ниже 4,5:1.
        "--btn-danger-bg": "#b4545c",
        "--btn-danger-hover": "#bf5f67",
        "--btn-danger-pressed": "#9e4850",
        "--btn-danger-glow": "rgba(180, 84, 92, 0.3)",
        "--btn-muted-bg": "#2b1f21",
        "--btn-muted-hover": "#302225",
      }
    ),
  },
];

export function themeById(id) {
  return THEMES.find((theme) => theme.id === id) || THEMES[0];
}

// Переменные темы вместе с выбранным акцентом; у темы без вариантов — свои.
export function themeVars(id, accentId) {
  const theme = themeById(id);
  if (!theme.accents) return theme.vars;
  const chosen = theme.accents.find((item) => item.id === accentId) || theme.accents[0];
  return Object.assign({}, theme.vars, chosen.vars);
}
