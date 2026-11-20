"use strict";

// Импорт конфига HL1 и настройки лаунчера. hlsr-personalization.cfg
// выполняется после импорта (и ещё раз в его хвосте, см.
// game-binds-install.js) и пишет прицел BXT, цвет HUD, панели BXT и поле
// зрения явно, даже по умолчанию. Без переноса значения игрока из его cfg
// молча заменялись бы значениями лаунчера. Поэтому при импорте они
// становятся настройками лаунчера: окно показывает их, лаунчер их и пишет.
//
// Берутся только команды верхнего уровня (их выполнит игра при запуске),
// последнее значение главнее. Значения внутри alias и биндов — это то, что
// игрок переключает клавишей, их лаунчер не трогает.

const gameBinds = require("./game-binds");

const PANELS = ["timer", "speedometer", "jumpspeed", "viewangles"];

// Последние значения переменных верхнего уровня: { имя (нижний регистр): значение }.
function cvarsOf(text) {
  const result = Object.create(null);
  for (const args of gameBinds.parseCfg(text)) {
    if (args.length < 2) continue;
    const verb = args[0].toLowerCase();
    if (verb === "bind" || verb === "alias" || verb === "exec" || verb === "unbind") continue;
    result[verb] = args[1];
  }
  return result;
}

const has = (cvars, name) => Object.prototype.hasOwnProperty.call(cvars, name);

function numberOf(value) {
  const text = String(value).trim();
  if (!text) return null;
  const n = Number(text);
  return Number.isFinite(n) ? n : null;
}

function colorOf(value) {
  const parts = String(value).trim().split(/\s+/);
  if (parts.length !== 3) return null;
  const n = parts.map(Number);
  if (!n.every((v) => Number.isFinite(v) && v >= 0 && v <= 255)) return null;
  return { r: Math.round(n[0]), g: Math.round(n[1]), b: Math.round(n[2]) };
}

// Переносит значения из text в копии personalization и bxt. Возвращает их
// и списки: adopted — перенесённые переменные, skipped — те, что лаунчер
// записать так не может (значение вне его пределов или сочетание, которого
// нет в настройках); их значение из импорта лаунчер перекроет.
function adoptImported(text, { personalization = {}, bxt = {} } = {}) {
  const cvars = cvarsOf(text);
  const p = Object.assign({}, personalization);
  const b = Object.assign({}, bxt);
  const adopted = [];
  const skipped = [];
  const take = (name, apply) => {
    if (!has(cvars, name)) return;
    if (apply(cvars[name]) === false) skipped.push(`${name} ${cvars[name]}`);
    else adopted.push(name);
  };
  const bool = (value) => {
    const n = numberOf(value);
    return n === null ? null : n !== 0;
  };
  // Число в пределах лаунчера; иначе false — в skipped.
  const ranged = (value, min, max, set) => {
    const n = numberOf(value);
    if (n === null || n < min || n > max) return false;
    set(Math.round(n));
    return true;
  };
  const flag = (set) => (value) => {
    const v = bool(value);
    if (v === null) return false;
    set(v);
    return true;
  };

  for (const key of PANELS) take(`bxt_hud_${key}`, flag((v) => { b[key] = v; }));
  take("default_fov", (v) => ranged(v, 70, 120, (n) => { b.fov = n; }));
  take("bxt_fix_widescreen_fov", flag((v) => { b.fixWidescreenFov = v; }));
  take("bxt_viewmodel_fov", (v) => ranged(v, 0, 130, (n) => { b.viewmodelFov = n; }));
  take("bxt_viewmodel_semitransparent", flag((v) => { b.viewmodelSemitransparent = v; }));
  take("bxt_remove_viewmodel", flag((v) => { b.removeViewmodel = v; }));
  // Смещение модели оружия: -20..20, как в окне настроек.
  take("bxt_viewmodel_ofs_forward", (v) => ranged(v, -20, 20, (n) => { b.viewmodelForward = n; }));
  take("bxt_viewmodel_ofs_right", (v) => ranged(v, -20, 20, (n) => { b.viewmodelRight = n; }));
  take("bxt_viewmodel_ofs_up", (v) => ranged(v, -20, 20, (n) => { b.viewmodelUp = n; }));
  take("bxt_viewmodel_bob_angled", flag((v) => { b.viewmodelBobAngled = v; }));
  take("bxt_viewmodel_disable_idle", flag((v) => { b.viewmodelNoIdle = v; }));
  take("bxt_viewmodel_disable_equip", flag((v) => { b.viewmodelNoEquip = v; }));
  take("bxt_hud_jumpdistance", flag((v) => { b.jumpdistance = v; }));
  take("bxt_hud_precision", (v) => ranged(v, 0, 16, (n) => { b.precision = n; }));
  take("bxt_hud_incorrect_fps_indicator", flag((v) => { b.incorrectFps = v; }));
  // Положение панели: "x y" — доли экрана, в окне проценты 0..100.
  for (const key of PANELS) {
    take(`bxt_hud_${key}_anchor`, (value) => {
      const [x, y] = String(value).trim().split(/\s+/).map(numberOf);
      if (x == null || y == null || x < 0 || x > 1 || y < 0 || y > 1) return false;
      b[key + "X"] = Math.round(x * 100);
      b[key + "Y"] = Math.round(y * 100);
      return true;
    });
  }

  // Цвет HUD: лаунчер пишет один цвет в bxt_hud_color и bxt_hud_game_color.
  const hudName = has(cvars, "bxt_hud_color") ? "bxt_hud_color" : "bxt_hud_game_color";
  take(hudName, (value) => {
    if (!String(value).trim()) { p.hudEnabled = false; return true; }
    const color = colorOf(value);
    if (!color) return false;
    Object.assign(p, { hudEnabled: true, hudColor: color, hudAlpha: -1, hudAffectsCrosshair: false });
    return true;
  });
  if (hudName === "bxt_hud_color" && has(cvars, "bxt_hud_game_color") && cvars.bxt_hud_game_color !== cvars.bxt_hud_color)
    skipped.push(`bxt_hud_game_color ${cvars.bxt_hud_game_color}`);
  // Прозрачность без своего цвета лаунчер не пишет (он всегда пишет 0).
  if (has(cvars, "bxt_hud_game_alpha") && numberOf(cvars.bxt_hud_game_alpha) !== 0)
    skipped.push(`bxt_hud_game_alpha ${cvars.bxt_hud_game_alpha}`);

  // Прицел BXT. Любая его настройка из cfg делает прицел «своим».
  const crossBefore = adopted.length + skipped.length;
  take("bxt_cross", flag((v) => { p.crossEnabled = v; }));
  take("bxt_cross_color", (value) => {
    const color = colorOf(value);
    if (!color) return false;
    p.crossColor = color;
    p.hudAffectsCrosshair = false;
    return true;
  });
  take("bxt_cross_alpha", (v) => ranged(v, 0, 255, (n) => { p.crossAlpha = n; }));
  take("bxt_cross_thickness", (v) => ranged(v, 1, 10, (n) => { p.crossThickness = n; }));
  take("bxt_cross_size", (v) => ranged(v, 0, 40, (n) => { p.crossSize = n; }));
  take("bxt_cross_gap", (v) => ranged(v, -30, 30, (n) => { p.crossGap = n; }));
  take("bxt_cross_outline", (v) => ranged(v, 0, 4, (n) => {
    p.crossOutline = n > 0;
    if (n > 0) p.crossOutlineThickness = n;
  }));
  take("bxt_cross_circle_radius", (v) => ranged(v, 0, 40, (n) => { p.crossCircleRadius = n; }));
  take("bxt_cross_dot_color", (value) => {
    if (!String(value).trim()) { p.crossDotColor = null; return true; }
    const color = colorOf(value);
    if (!color) return false;
    p.crossDotColor = color;
    return true;
  });
  take("bxt_cross_dot_size", (v) => ranged(v, 0, 12, (n) => { p.crossDotSize = n; }));
  take("bxt_cross_top_line", flag((v) => { p.crossTopLine = v; }));
  take("bxt_cross_bottom_line", flag((v) => { p.crossBottomLine = v; }));
  take("bxt_cross_left_line", flag((v) => { p.crossLeftLine = v; }));
  take("bxt_cross_right_line", flag((v) => { p.crossRightLine = v; }));
  if (adopted.length + skipped.length > crossBefore && p.crossEnabled) p.crossPreset = "custom";
  else if (has(cvars, "bxt_cross") && !p.crossEnabled) p.crossPreset = "standard";

  // Штатный прицел: с прицелом BXT его скрывает настройка прицела, без
  // него — отдельный флаг bxt.hideCrosshair.
  take("crosshair", (value) => {
    const on = bool(value);
    if (on === null) return false;
    b.hideCrosshair = !on && !p.crossEnabled;
    if (p.crossEnabled) p.crossHideDefault = !on;
    return true;
  });

  return { personalization: p, bxt: b, adopted, skipped };
}

module.exports = { cvarsOf, adoptImported };
