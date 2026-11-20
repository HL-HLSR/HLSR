"use strict";

// Настройки кастомизации HL1 (WON 2005 + Bunnymod XT), которые не входят в
// редактор цвета HUD и прицела. Формат полей тот же, что у ghosting-settings,
// поэтому окно настроек показывает их тем же списком.
//
// Все команды из белого списка правил HL1 на speedrun.com. Значения по
// умолчанию — как у BXT (cvars.hpp): спидометр у него включён изначально.

const CHANGER_SHAPES = [
  "Default",
  "1px",
  "3px",
  "Small Circle",
  "Medium Circle",
  "Large Circle",
  "Small Circle with 1px",
  "Medium Circle with 1px",
  "Large Circle with 1px",
];
const CHANGER_COLORS = ["green", "red", "blue", "lightblue", "orange", "pink", "yellow"];

const HUD_PACKS = ["default", "ag-hud"];
const CROSSHAIR_PACKS = ["default", "execut4ble-crosshair", "crosshair-changer"];

const option = (value, label) => ({ value, label });

// Положение панели BXT на экране: bxt_hud_<panel>_anchor — доля ширины и
// высоты экрана, в процентах она не зависит от разрешения. Сдвиг панели от
// этой точки (bxt_hud_<panel>_offset) остаётся стандартным, BXT сам ставит,
// например, спидометр на две строки выше точки. По умолчанию — как в BXT
// (cvars.hpp). Оба cvar разрешены правилами GoldSrc: в whitelist у них нет
// обязательного значения.
const position = (panel, x, y) => [
  { key: panel + "X", type: "number", default: x, label: "#L_GS_H_POS_X", hint: "bxt_hud_" + panel + "_anchor", min: 0, max: 100, step: 1, requires: panel },
  { key: panel + "Y", type: "number", default: y, label: "#L_GS_H_POS_Y", hint: "bxt_hud_" + panel + "_anchor", min: 0, max: 100, step: 1, requires: panel },
];

const SECTIONS = [
  { id: "hl1packs", title: "#L_GS_H_S_PACKS", icon: "fa-box-open", link: "hl1Resources", fields: [
    { key: "packHud", type: "select", default: "default", label: "#L_GS_H_PACK_HUD", hint: "#L_GS_H_PACK_HUD_HINT",
      options: [option("default", "#L_GS_H_PACK_NONE"), option("ag-hud", "#L_GS_H_PACK_AG_HUD")] },
    { key: "packCrosshair", type: "select", default: "default", label: "#L_GS_H_PACK_CROSS", hint: "#L_GS_H_PACK_CROSS_HINT",
      options: [
        option("default", "#L_GS_H_PACK_NONE"),
        option("execut4ble-crosshair", "#L_GS_H_PACK_E4_CROSS"),
        option("crosshair-changer", "#L_GS_H_PACK_CHANGER"),
      ] },
    { key: "changerShape", type: "select", default: "Default", label: "#L_GS_H_CHANGER_SHAPE",
      requires: "packCrosshair", requiresValue: "crosshair-changer",
      options: CHANGER_SHAPES.map((shape) => option(shape, shape)) },
    { key: "changerColor", type: "select", default: "green", label: "#L_GS_H_CHANGER_COLOR",
      requires: "packCrosshair", requiresValue: "crosshair-changer",
      options: CHANGER_COLORS.map((color) => option(color, "#L_GS_H_COLOR_" + color.toUpperCase())) },
  ] },
  { id: "hl1hud", title: "#L_GS_H_S_HUD", icon: "fa-stopwatch", fields: [
    { key: "timer", type: "boolean", default: false, label: "#L_GS_H_TIMER", hint: "bxt_hud_timer" },
    ...position("timer", 0, 50),
    { key: "speedometer", type: "boolean", default: true, label: "#L_GS_H_SPEEDOMETER", hint: "bxt_hud_speedometer" },
    ...position("speedometer", 50, 100),
    { key: "jumpspeed", type: "boolean", default: false, label: "#L_GS_H_JUMPSPEED", hint: "bxt_hud_jumpspeed" },
    ...position("jumpspeed", 50, 100),
    { key: "viewangles", type: "boolean", default: false, label: "#L_GS_H_VIEWANGLES", hint: "bxt_hud_viewangles" },
    ...position("viewangles", 100, 0),
    { key: "jumpdistance", type: "boolean", default: false, label: "#L_GS_H_JUMPDISTANCE", hint: "bxt_hud_jumpdistance" },
    { key: "precision", type: "number", default: 6, label: "#L_GS_H_PRECISION", hint: "#L_GS_H_PRECISION_HINT", min: 0, max: 16, step: 1 },
    { key: "incorrectFps", type: "boolean", default: true, label: "#L_GS_H_BAD_FPS", hint: "#L_GS_H_BAD_FPS_HINT" },
  ] },
  { id: "hl1view", title: "#L_GS_H_S_VIEW", icon: "fa-eye", fields: [
    { key: "fov", type: "number", default: 90, label: "#L_GS_H_FOV", hint: "default_fov", min: 70, max: 120, step: 1 },
    { key: "fixWidescreenFov", type: "boolean", default: false, label: "#L_GS_H_WIDESCREEN", hint: "#L_GS_H_WIDESCREEN_HINT" },
    { key: "viewmodelFov", type: "number", default: 0, label: "#L_GS_H_VM_FOV", hint: "#L_GS_H_VM_FOV_HINT", min: 0, max: 130, step: 1 },
    { key: "viewmodelSemitransparent", type: "boolean", default: false, label: "#L_GS_H_VM_SEMI", hint: "bxt_viewmodel_semitransparent" },
    { key: "removeViewmodel", type: "boolean", default: false, label: "#L_GS_H_VM_REMOVE", hint: "bxt_remove_viewmodel" },
    { key: "viewmodelForward", type: "number", default: 0, label: "#L_GS_H_VM_FORWARD", hint: "bxt_viewmodel_ofs_forward", min: -20, max: 20, step: 1 },
    { key: "viewmodelRight", type: "number", default: 0, label: "#L_GS_H_VM_RIGHT", hint: "bxt_viewmodel_ofs_right", min: -20, max: 20, step: 1 },
    { key: "viewmodelUp", type: "number", default: 0, label: "#L_GS_H_VM_UP", hint: "bxt_viewmodel_ofs_up", min: -20, max: 20, step: 1 },
    { key: "viewmodelBobAngled", type: "boolean", default: false, label: "#L_GS_H_VM_BOB", hint: "bxt_viewmodel_bob_angled" },
    { key: "viewmodelNoIdle", type: "boolean", default: false, label: "#L_GS_H_VM_NO_IDLE", hint: "bxt_viewmodel_disable_idle" },
    { key: "viewmodelNoEquip", type: "boolean", default: false, label: "#L_GS_H_VM_NO_EQUIP", hint: "bxt_viewmodel_disable_equip" },
    { key: "hideCrosshair", type: "boolean", default: false, label: "#L_GS_H_HIDE_CROSS", hint: "#L_GS_H_HIDE_CROSS_HINT" },
  ] },
];

const FIELDS = SECTIONS.reduce((all, section) => all.concat(section.fields), []);
const BY_KEY = Object.create(null);
FIELDS.forEach((field) => { BY_KEY[field.key] = field; });
const own = (value, key) => Object.prototype.hasOwnProperty.call(value, key);

const BXT_KEYS = FIELDS.filter((field) => !/^pack|^changer/.test(field.key)).map((field) => field.key);

// Поле с requiresValue живо, только пока у поля requires именно это значение:
// форма и цвет из Crosshair Changer ничего не значат для других прицелов.
function available(field, values) {
  if (!field.requires) return true;
  const value = own(values, field.requires) ? values[field.requires] : BY_KEY[field.requires].default;
  return field.requiresValue !== undefined ? value === field.requiresValue : !!value && value !== "0";
}

function sanitizeValue(field, value) {
  if (field.type === "boolean") return typeof value === "boolean" ? value : undefined;
  if (field.type === "number") {
    if (typeof value === "string" && !value.trim()) return undefined;
    const numeric = Number(value);
    if ((typeof value !== "number" && typeof value !== "string") || !Number.isFinite(numeric)) return undefined;
    return Math.round(Math.max(field.min, Math.min(field.max, numeric)));
  }
  return field.options.some((item) => item.value === value) ? value : undefined;
}

function defaults() {
  const result = {};
  FIELDS.forEach((field) => { result[field.key] = field.default; });
  return result;
}

function sanitizeExtras(input) {
  const result = {};
  if (!input || typeof input !== "object" || Array.isArray(input)) return result;
  FIELDS.forEach((field) => {
    if (!own(input, field.key)) return;
    const clean = sanitizeValue(field, input[field.key]);
    if (clean !== undefined) result[field.key] = clean;
  });
  return result;
}

function resetExtras(input) {
  const result = {};
  Object.keys(sanitizeExtras(input)).forEach((key) => { result[key] = BY_KEY[key].default; });
  return result;
}

// Хранятся настройки двумя полями: packs (его читает установщик паков) и bxt
// (его пишет cfg). В окне настроек они — один плоский список.
function toFlat(packs, bxt) {
  const p = packs || {};
  return Object.assign({}, sanitizeExtras(bxt), sanitizeExtras({
    packHud: p.hud,
    packCrosshair: p.crosshair,
    changerShape: p.changerShape,
    changerColor: p.changerColor,
  }));
}

function fromFlat(flat) {
  const clean = Object.assign(defaults(), sanitizeExtras(flat));
  const bxt = {};
  BXT_KEYS.forEach((key) => { bxt[key] = clean[key]; });
  return {
    packs: { hud: clean.packHud, crosshair: clean.packCrosshair, changerShape: clean.changerShape, changerColor: clean.changerColor },
    bxt,
  };
}

module.exports = {
  CHANGER_SHAPES,
  CHANGER_COLORS,
  HUD_PACKS,
  CROSSHAIR_PACKS,
  SECTIONS,
  FIELDS,
  BXT_KEYS,
  defaults,
  available,
  sanitizeExtras,
  resetExtras,
  toFlat,
  fromFlat,
};
