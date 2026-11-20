"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const extraSettings = require("../../shared/ghosting-settings");
const bindPresets = require("../../shared/bind-presets");
const DEFAULT_AUTOEXEC = require("./ghosting-default-autoexec");

const PERSONALIZATION_FILE = "hlsr-personalization.cfg";
const GAME_DIR = "ghostingmod";
const CFG_DIR = "ghostingmod/cfg";
const EXTRAS_MARKER = "// HLSR_EXTRA_SETTINGS_VERSION 1";

const HUD_PRESETS = {
  stock: { r: 255, g: 220, b: 0 },
  white: { r: 255, g: 255, b: 255 },
  green: { r: 0, g: 255, b: 0 },
  cyan: { r: 0, g: 255, b: 255 },
  blue: { r: 80, g: 160, b: 255 },
  orange: { r: 255, g: 128, b: 0 },
  pink: { r: 255, g: 80, b: 180 },
  purple: { r: 180, g: 80, b: 255 },
};

// Пресеты прицела заданы явными значениями, а не командами
// gh_cross_preset_*, чтобы предпросмотр в лаунчере совпадал с игрой 1:1.
const CROSS_PRESETS = {
  standard: null,
  custom: null,
  dot: {
    crossEnabled: true,
    crossColor: { r: 0, g: 255, b: 0 },
    crossAlpha: 255,
    thickness: 2,
    size: 0,
    gap: 0,
    outline: 1,
    outlineThickness: 1,
    circleRadius: 0,
    dotColor: null,
    dotSize: 4,
    topLine: false,
    bottomLine: false,
    leftLine: false,
    rightLine: false,
    hideDefault: true,
  },
  classic: {
    crossEnabled: true,
    crossColor: { r: 0, g: 255, b: 0 },
    crossAlpha: 255,
    thickness: 2,
    size: 10,
    gap: 3,
    outline: 0,
    outlineThickness: 1,
    circleRadius: 0,
    dotColor: null,
    dotSize: 0,
    topLine: true,
    bottomLine: true,
    leftLine: true,
    rightLine: true,
    hideDefault: true,
  },
  circle: {
    crossEnabled: true,
    crossColor: { r: 0, g: 255, b: 0 },
    crossAlpha: 255,
    thickness: 2,
    size: 0,
    gap: 0,
    outline: 1,
    outlineThickness: 1,
    circleRadius: 12,
    dotColor: null,
    dotSize: 2,
    topLine: false,
    bottomLine: false,
    leftLine: false,
    rightLine: false,
    hideDefault: true,
  },
  crossdot: {
    crossEnabled: true,
    crossColor: { r: 255, g: 255, b: 0 },
    crossAlpha: 255,
    thickness: 2,
    size: 8,
    gap: 4,
    outline: 1,
    outlineThickness: 1,
    circleRadius: 0,
    dotColor: null,
    dotSize: 2,
    topLine: true,
    bottomLine: true,
    leftLine: true,
    rightLine: true,
    hideDefault: true,
  },
};

// Preset ids arrive from stored settings and the renderer. Inherited names such
// as "constructor" or "__proto__" must not pass as presets.
function isCrossPreset(id) {
  return typeof id === "string" && Object.prototype.hasOwnProperty.call(CROSS_PRESETS, id);
}

function clampInt(value, min, max, fallback) {
  const n = typeof value === "string" && value.trim() !== "" ? Number(value) : value;
  if (!Number.isFinite(n)) return fallback;
  const i = Math.round(n);
  if (i < min) return min;
  if (i > max) return max;
  return i;
}

function sanitizeColor(value, fallback) {
  const fb = fallback || { r: 255, g: 220, b: 0 };
  if (!value || typeof value !== "object") return { r: fb.r, g: fb.g, b: fb.b };
  return {
    r: clampInt(value.r, 0, 255, fb.r),
    g: clampInt(value.g, 0, 255, fb.g),
    b: clampInt(value.b, 0, 255, fb.b),
  };
}

function colorToString(color) {
  return `${color.r} ${color.g} ${color.b}`;
}

function defaultPersonalization() {
  return {
    hudEnabled: false,
    hudColor: { r: 255, g: 220, b: 0 },
    hudAlpha: -1,
    hudAffectsCrosshair: false,
    hideQuickInfo: false,
    crossPreset: "standard",
    crossEnabled: false,
    crossColor: { r: 0, g: 255, b: 0 },
    crossAlpha: 255,
    crossThickness: 2,
    crossSize: 10,
    crossGap: 3,
    crossOutline: false,
    crossOutlineThickness: 1,
    crossCircleRadius: 0,
    crossDotColor: null,
    crossDotSize: 0,
    crossTopLine: true,
    crossBottomLine: true,
    crossLeftLine: true,
    crossRightLine: true,
    crossHideDefault: true,
    extra: {},
  };
}

function sanitizePersonalization(input) {
  const src = input && typeof input === "object" ? input : {};
  const def = defaultPersonalization();

  const hudColor = sanitizeColor(src.hudColor, def.hudColor);
  const crossColor = sanitizeColor(src.crossColor, def.crossColor);

  let crossDotColor = null;
  if (src.crossDotColor && typeof src.crossDotColor === "object") {
    crossDotColor = sanitizeColor(src.crossDotColor, crossColor);
    // Пустая строка в игре означает «как основной цвет».
    // В настройках это представлено null; объект считаем заданным цветом.
  } else if (typeof src.crossDotColor === "string" && src.crossDotColor.trim() !== "") {
    const m = src.crossDotColor.trim().split(/\s+/).map(Number);
    if (m.length === 3 && m.every(Number.isFinite)) {
      crossDotColor = {
        r: clampInt(m[0], 0, 255, crossColor.r),
        g: clampInt(m[1], 0, 255, crossColor.g),
        b: clampInt(m[2], 0, 255, crossColor.b),
      };
    }
  }

  const preset = isCrossPreset(src.crossPreset) ? src.crossPreset : "standard";

  return {
    hudEnabled: src.hudEnabled === true,
    hudColor,
    hudAlpha: clampInt(src.hudAlpha, -1, 255, -1),
    hudAffectsCrosshair: src.hudAffectsCrosshair === true,
    hideQuickInfo: src.hideQuickInfo === true,
    crossPreset: preset,
    crossEnabled: src.crossEnabled === true,
    crossColor,
    crossAlpha: clampInt(src.crossAlpha, 0, 255, 255),
    crossThickness: clampInt(src.crossThickness, 1, 10, 2),
    crossSize: clampInt(src.crossSize, 0, 40, 10),
    // Отрицательный зазор принимает только BXT (HL1): линии заходят за
    // центр. Ghosting пишет его не меньше 0, см. toCfgLines.
    crossGap: clampInt(src.crossGap, -30, 30, 3),
    crossOutline: src.crossOutline === true,
    crossOutlineThickness: clampInt(src.crossOutlineThickness, 1, 4, 1),
    crossCircleRadius: clampInt(src.crossCircleRadius, 0, 40, 0),
    crossDotColor,
    crossDotSize: clampInt(src.crossDotSize, 0, 12, 0),
    crossTopLine: src.crossTopLine !== false,
    crossBottomLine: src.crossBottomLine !== false,
    crossLeftLine: src.crossLeftLine !== false,
    crossRightLine: src.crossRightLine !== false,
    crossHideDefault: src.crossHideDefault !== false,
    extra: extraSettings.sanitizeExtras(src.extra),
  };
}

function applyPreset(base, presetId) {
  const clean = sanitizePersonalization(base);
  if (presetId === "standard") {
    clean.crossPreset = "standard";
    clean.crossEnabled = false;
    return clean;
  }
  const preset = isCrossPreset(presetId) ? CROSS_PRESETS[presetId] : null;
  if (!preset) return clean;
  return Object.assign({}, clean, {
    crossPreset: presetId,
    crossEnabled: preset.crossEnabled,
    crossColor: Object.assign({}, preset.crossColor),
    crossAlpha: preset.crossAlpha,
    crossThickness: preset.thickness,
    crossSize: preset.size,
    crossGap: preset.gap,
    crossOutline: !!preset.outline,
    crossOutlineThickness: preset.outlineThickness,
    crossCircleRadius: preset.circleRadius,
    crossDotColor: preset.dotColor ? Object.assign({}, preset.dotColor) : null,
    crossDotSize: preset.dotSize,
    crossTopLine: !!preset.topLine,
    crossBottomLine: !!preset.bottomLine,
    crossLeftLine: !!preset.leftLine,
    crossRightLine: !!preset.rightLine,
    crossHideDefault: !!preset.hideDefault,
  });
}

function boolToNum(value) {
  return value ? "1" : "0";
}

function toCfgLines(settings) {
  const s = sanitizePersonalization(settings);
  const effectiveCrossColor = s.hudAffectsCrosshair ? s.hudColor : s.crossColor;
  const lines = [];
  lines.push("// Generated by HLSR Launcher - Personalization. Do not edit manually.");
  lines.push("// ghostingmod/cfg/hlsr-personalization.cfg - applied via autoexec.cfg");
  lines.push(`gh_hud_color "${colorToString(s.hudColor)}"`);
  lines.push(`gh_hud_color_enable "${boolToNum(s.hudEnabled)}"`);
  lines.push(`gh_hud_color_alpha "${s.hudAlpha}"`);
  lines.push(`gh_hud_color_affects_crosshair "${boolToNum(s.hudAffectsCrosshair)}"`);
  lines.push(`hud_quickinfo "${s.hideQuickInfo ? "0" : "1"}"`);
  lines.push(`gh_cross "${boolToNum(s.crossEnabled)}"`);
  lines.push(`gh_cross_color "${colorToString(effectiveCrossColor)}"`);
  lines.push(`gh_cross_alpha "${s.crossAlpha}"`);
  lines.push(`gh_cross_thickness "${s.crossThickness}"`);
  lines.push(`gh_cross_size "${s.crossSize}"`);
  lines.push(`gh_cross_gap "${Math.max(0, s.crossGap)}"`);
  lines.push(`gh_cross_outline "${boolToNum(s.crossOutline)}"`);
  lines.push(`gh_cross_outline_thickness "${s.crossOutlineThickness}"`);
  lines.push(`gh_cross_circle_radius "${s.crossCircleRadius}"`);
  lines.push(`gh_cross_dot_color "${s.crossDotColor ? colorToString(s.crossDotColor) : ""}"`);
  lines.push(`gh_cross_dot_size "${s.crossDotSize}"`);
  lines.push(`gh_cross_top_line "${boolToNum(s.crossTopLine)}"`);
  lines.push(`gh_cross_bottom_line "${boolToNum(s.crossBottomLine)}"`);
  lines.push(`gh_cross_left_line "${boolToNum(s.crossLeftLine)}"`);
  lines.push(`gh_cross_right_line "${boolToNum(s.crossRightLine)}"`);
  lines.push(`gh_cross_hide_default "${boolToNum(s.crossHideDefault)}"`);
  lines.push(EXTRAS_MARKER);
  const extraLines = extraSettings.serializeExtras(s.extra);
  if (extraLines) lines.push(extraLines);
  lines.push("");
  return lines.join("\n");
}

function personalizationFile(installPath) {
  return path.join(installPath, CFG_DIR, PERSONALIZATION_FILE);
}

// Стоковый autoexec.cfg сборки (база 1.0.2–1.0.7) без строк exec лаунчера.
const STOCK_AUTOEXEC_SHA256 = new Set([
  "8a2f1730360d8a8c4f3bb964144ee9f63a4bb94e6aba5afd5b5372cc3a8c94ba",
]);
const LAUNCHER_EXEC_LINE = /^exec\s+hlsr-[a-z-]+\.cfg$/i;

// Бинд удаления сейва из стандартного autoexec под Windows. Под Linux он не
// работает, поэтому в autoexec, поставленном лаунчером раньше, меняется на
// рабочий. Остальное в файле игрока не трогается.
const WINDOWS_SAVE_DELETION_BIND = 'bind h "save :; reload"';

function fixSaveDeletionBind(autoexec, text, platform) {
  if (platform !== "linux") return false;
  const fixed = `bind h "${bindPresets.GHOSTING_SAVE_DELETION}"`;
  const lines = text.split("\n");
  let changed = false;
  for (let i = 0; i < lines.length; i++) {
    const cr = lines[i].endsWith("\r") ? "\r" : "";
    if (lines[i].trim() !== WINDOWS_SAVE_DELETION_BIND) continue;
    lines[i] = lines[i].slice(0, lines[i].indexOf("bind")) + fixed + cr;
    changed = true;
  }
  if (changed) fs.writeFileSync(autoexec, lines.join("\n"), "utf8");
  return changed;
}

// Ставит стандартный autoexec лаунчера вместо отсутствующего или стокового.
// Свой autoexec игрока не трогает. Строки exec лаунчера переносятся.
function ensureDefaultAutoexec(installPath, options = {}) {
  const autoexec = path.join(installPath, CFG_DIR, "autoexec.cfg");
  let text = null;
  try {
    text = fs.readFileSync(autoexec, "utf8");
  } catch (error) {
    if (error && error.code !== "ENOENT") throw error;
  }
  const lines = text === null ? [] : text.replace(/\r\n/g, "\n").split("\n");
  const execLines = lines.map((line) => line.trim()).filter((line) => LAUNCHER_EXEC_LINE.test(line));
  if (text !== null) {
    const own = lines.filter((line) => !LAUNCHER_EXEC_LINE.test(line.trim())).join("\n").replace(/\s+$/, "");
    const hash = crypto.createHash("sha256").update(own, "utf8").digest("hex");
    if (!STOCK_AUTOEXEC_SHA256.has(hash)) {
      fixSaveDeletionBind(autoexec, text, options.platform || process.platform);
      return false;
    }
  }
  fs.mkdirSync(path.dirname(autoexec), { recursive: true });
  fs.writeFileSync(autoexec, DEFAULT_AUTOEXEC + execLines.map((line) => `${line}\r\n`).join(""), "utf8");
  return true;
}

// Чтение config.cfg для первичного импорта: если игрок уже настроил
// цвет/прицел через консоль, подхватим его как стартовые значения.
function parseCfgValue(text, name) {
  if (typeof text !== "string") return null;
  const lower = String(name).toLowerCase();
  const lines = text.split(/\r?\n/);
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("//")) continue;
    const m = /^\s*([A-Za-z0-9_.]+)\s+(.*)\s*$/.exec(line);
    if (!m) continue;
    if (m[1].toLowerCase() !== lower) continue;
    let val = m[2].trim();
    // Убираем trailing-комментарий вида: gh_cross "0" // comment
    // (кавычки учитываем грубо: режем по // вне кавычек).
    let inQuotes = false;
    for (let i = 0; i < val.length - 1; i++) {
      if (val[i] === '"') inQuotes = !inQuotes;
      if (!inQuotes && val[i] === "/" && val[i + 1] === "/") {
        val = val.slice(0, i).trim();
        break;
      }
    }
    if (val.length >= 2 && val.startsWith('"') && val.endsWith('"')) {
      return val.slice(1, -1);
    }
    return val;
  }
  return null;
}

function parseColorString(str, fallback) {
  if (typeof str !== "string" || !str.trim()) return fallback;
  const parts = str.trim().split(/\s+/).map(Number);
  if (parts.length !== 3 || !parts.every(Number.isFinite)) return fallback;
  return {
    r: clampInt(parts[0], 0, 255, fallback.r),
    g: clampInt(parts[1], 0, 255, fallback.g),
    b: clampInt(parts[2], 0, 255, fallback.b),
  };
}

// options.text — читать этот текст (файл игрока) вместо config.cfg.
function importFromConfigCfg(installPath, options = {}) {
  try {
    const given = typeof options.text === "string";
    // Импорт нужен ровно один раз. После первого сохранения отдельный файл
    // становится источником истины и старые значения из config.cfg больше
    // не должны отменять сброс пользователя к настройкам по умолчанию.
    if (!given && fs.existsSync(personalizationFile(installPath))) return null;
    const text = given ? options.text : fs.readFileSync(path.join(installPath, CFG_DIR, "config.cfg"), "utf8");
    const knownKeys = [
      "gh_hud_color",
      "gh_hud_color_enable",
      "hud_quickinfo",
      "gh_cross",
      "gh_cross_color",
    ];
    const extra = extraSettings.importExtras(text);
    if (!knownKeys.some((key) => parseCfgValue(text, key) !== null) && !Object.keys(extra).length) return null;
    const def = defaultPersonalization();
    const hudColor = parseColorString(parseCfgValue(text, "gh_hud_color"), def.hudColor);
    const crossColor = parseColorString(parseCfgValue(text, "gh_cross_color"), def.crossColor);
    const dotRaw = parseCfgValue(text, "gh_cross_dot_color");
    return sanitizePersonalization({
      hudEnabled: parseCfgValue(text, "gh_hud_color_enable") === "1",
      hudColor,
      hudAlpha: parseCfgValue(text, "gh_hud_color_alpha"),
      hudAffectsCrosshair: parseCfgValue(text, "gh_hud_color_affects_crosshair") === "1",
      hideQuickInfo: parseCfgValue(text, "hud_quickinfo") === "0",
      crossEnabled: parseCfgValue(text, "gh_cross") === "1",
      crossColor,
      crossAlpha: parseCfgValue(text, "gh_cross_alpha"),
      crossThickness: parseCfgValue(text, "gh_cross_thickness"),
      crossSize: parseCfgValue(text, "gh_cross_size"),
      crossGap: parseCfgValue(text, "gh_cross_gap"),
      crossOutline: parseCfgValue(text, "gh_cross_outline") === "1",
      crossOutlineThickness: parseCfgValue(text, "gh_cross_outline_thickness"),
      crossCircleRadius: parseCfgValue(text, "gh_cross_circle_radius"),
      crossDotColor: dotRaw && dotRaw.trim() ? dotRaw : null,
      crossDotSize: parseCfgValue(text, "gh_cross_dot_size"),
      crossTopLine: (parseCfgValue(text, "gh_cross_top_line") || "1") === "1",
      crossBottomLine: (parseCfgValue(text, "gh_cross_bottom_line") || "1") === "1",
      crossLeftLine: (parseCfgValue(text, "gh_cross_left_line") || "1") === "1",
      crossRightLine: (parseCfgValue(text, "gh_cross_right_line") || "1") === "1",
      crossHideDefault: (parseCfgValue(text, "gh_cross_hide_default") || "1") === "1",
      crossPreset: "custom",
      extra,
    });
  } catch (error) {
    return null;
  }
}

// Upgrading an existing installation imports the new settings without replacing
// its already-saved HUD/crosshair. The marker is written even for an empty map,
// so an explicit reset/clear can never revive stale config.cfg values.
function importExtrasFromConfigCfg(installPath, options = {}) {
  try {
    if (typeof options.text === "string") {
      const fromText = extraSettings.importExtras(options.text);
      return Object.keys(fromText).length ? fromText : null;
    }
    let ownText = "";
    try {
      ownText = fs.readFileSync(personalizationFile(installPath), "utf8");
    } catch (error) {
      if (!error || error.code !== "ENOENT") throw error;
    }
    if (ownText.split(/\r?\n/).some((line) => line.trim() === EXTRAS_MARKER)) return null;
    let gameText = "";
    try {
      gameText = fs.readFileSync(path.join(installPath, CFG_DIR, "config.cfg"), "utf8");
    } catch (error) {
      if (!error || error.code !== "ENOENT") throw error;
    }
    const extra = Object.assign({}, extraSettings.importExtras(gameText), extraSettings.importExtras(ownText));
    return Object.keys(extra).length ? extra : null;
  } catch (error) {
    return null;
  }
}

module.exports = {
  PERSONALIZATION_FILE,
  GAME_DIR,
  CFG_DIR,
  EXTRAS_MARKER,
  HUD_PRESETS,
  CROSS_PRESETS,
  defaultPersonalization,
  sanitizePersonalization,
  applyPreset,
  toCfgLines,
  ensureDefaultAutoexec,
  STOCK_AUTOEXEC_SHA256,
  WINDOWS_SAVE_DELETION_BIND,
  importFromConfigCfg,
  importExtrasFromConfigCfg,
  colorToString,
};
