"use strict";

// Verified against Ghosting Mod Renewed 3.9 client/server ConVars and command
// help, including constructor defaults, except cl_showfps and cl_showpos: the
// launcher's default autoexec turns both on. Preview defaults are never written
// unless a setting is explicitly changed or reset by the user. Bounds without
// engine limits are conservative editor limits in 640 x 480 HUD layout units.
const ANCHOR_OPTIONS = [
  {
    "value": "0",
    "label": "#L_GS_X_ANCHOR_0"
  },
  {
    "value": "1",
    "label": "#L_GS_X_ANCHOR_1"
  },
  {
    "value": "2",
    "label": "#L_GS_X_ANCHOR_2"
  },
  {
    "value": "3",
    "label": "#L_GS_X_ANCHOR_3"
  },
  {
    "value": "4",
    "label": "#L_GS_X_ANCHOR_4"
  },
  {
    "value": "5",
    "label": "#L_GS_X_ANCHOR_5"
  },
  {
    "value": "6",
    "label": "#L_GS_X_ANCHOR_6"
  },
  {
    "value": "7",
    "label": "#L_GS_X_ANCHOR_7"
  },
  {
    "value": "8",
    "label": "#L_GS_X_ANCHOR_8"
  },
  {
    "value": "9",
    "label": "#L_GS_X_ANCHOR_9"
  }
];

const SECTIONS = [
  { id: "hud", icon: "◧", title: "#L_GS_X_SECTION_HUD", fields: [
    {"key": "gh_hud_font", "type": "text", "default": "", "label": "#L_GS_X_GH_HUD_FONT", "hint": "#L_GS_X_GH_HUD_FONT_HINT", "maxLength": 200},
    {"key": "gh_hud_fontscale", "type": "number", "default": 1, "label": "#L_GS_X_GH_HUD_FONTSCALE", "min": 0.25, "max": 4, "step": 0.05, "requires": "gh_hud_font"},
    {"key": "gh_hud_fontweight", "type": "number", "default": 0, "label": "#L_GS_X_GH_HUD_FONTWEIGHT", "min": 0, "max": 1000, "step": 100, "hint": "#L_GS_X_GH_HUD_FONTWEIGHT_HINT", "requires": "gh_hud_font"},
    {"key": "gh_hud_fontoutline", "type": "boolean", "default": false, "label": "#L_GS_X_GH_HUD_FONTOUTLINE", "requires": "gh_hud_font"},
    {"key": "gh_hud_bg", "type": "boolean", "default": true, "label": "#L_GS_X_GH_HUD_BG"},
    {"key": "gh_hud_labels", "type": "boolean", "default": true, "label": "#L_GS_X_GH_HUD_LABELS"},
    {"key": "gh_hud_icons", "type": "boolean", "default": true, "label": "#L_GS_X_GH_HUD_ICONS"},
  ] },
  { id: "layout", icon: "⊞", title: "#L_GS_X_SECTION_LAYOUT", fields: [
    {"key": "gh_health_align", "type": "select", "default": "0", "label": "#L_GS_X_GH_HEALTH_ALIGN", "options": ANCHOR_OPTIONS},
    {"key": "gh_health_x", "type": "number", "default": 0, "label": "#L_GS_X_GH_HEALTH_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_HEALTH_X_HINT"},
    {"key": "gh_health_y", "type": "number", "default": 0, "label": "#L_GS_X_GH_HEALTH_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_HEALTH_Y_HINT"},
    {"key": "gh_suit_align", "type": "select", "default": "0", "label": "#L_GS_X_GH_SUIT_ALIGN", "options": ANCHOR_OPTIONS},
    {"key": "gh_suit_x", "type": "number", "default": 0, "label": "#L_GS_X_GH_SUIT_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_SUIT_X_HINT"},
    {"key": "gh_suit_y", "type": "number", "default": 0, "label": "#L_GS_X_GH_SUIT_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_SUIT_Y_HINT"},
    {"key": "gh_ammo_align", "type": "select", "default": "0", "label": "#L_GS_X_GH_AMMO_ALIGN", "options": ANCHOR_OPTIONS},
    {"key": "gh_ammo_x", "type": "number", "default": 0, "label": "#L_GS_X_GH_AMMO_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_AMMO_X_HINT"},
    {"key": "gh_ammo_y", "type": "number", "default": 0, "label": "#L_GS_X_GH_AMMO_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_AMMO_Y_HINT"},
    {"key": "gh_ammo2_align", "type": "select", "default": "0", "label": "#L_GS_X_GH_AMMO2_ALIGN", "options": ANCHOR_OPTIONS},
    {"key": "gh_ammo2_x", "type": "number", "default": 0, "label": "#L_GS_X_GH_AMMO2_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_AMMO2_X_HINT"},
    {"key": "gh_ammo2_y", "type": "number", "default": 0, "label": "#L_GS_X_GH_AMMO2_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_AMMO2_Y_HINT"},
  ] },
  { id: "keys", icon: "⌨", title: "#L_GS_X_SECTION_KEYS", fields: [
    {"key": "gh_keyshud", "type": "boolean", "default": false, "label": "#L_GS_X_GH_KEYSHUD"},
    {"key": "gh_keyshud_full", "type": "boolean", "default": false, "label": "#L_GS_X_GH_KEYSHUD_FULL", "requires": "gh_keyshud"},
    {"key": "gh_keyshud_wheel", "type": "boolean", "default": true, "label": "#L_GS_X_GH_KEYSHUD_WHEEL", "requires": "gh_keyshud"},
    {"key": "gh_keyshud_mouse", "type": "boolean", "default": true, "label": "#L_GS_X_GH_KEYSHUD_MOUSE", "requires": "gh_keyshud"},
    {"key": "gh_keyshud_bg", "type": "boolean", "default": true, "label": "#L_GS_X_GH_KEYSHUD_BG", "requires": "gh_keyshud"},
    {"key": "gh_keyshud_size", "type": "number", "default": 13, "label": "#L_GS_X_GH_KEYSHUD_SIZE", "min": 6, "max": 64, "step": 1, "requires": "gh_keyshud"},
    {"key": "gh_keyshud_full_size", "type": "number", "default": 9, "label": "#L_GS_X_GH_KEYSHUD_FULL_SIZE", "min": 4, "max": 32, "step": 1, "requires": "gh_keyshud_full"},
    {"key": "gh_keyshud_padding", "type": "number", "default": 2, "label": "#L_GS_X_GH_KEYSHUD_PADDING", "min": 0, "max": 16, "step": 1, "requires": "gh_keyshud"},
    {"key": "gh_keyshud_mouse_scale", "type": "number", "default": 4, "label": "#L_GS_X_GH_KEYSHUD_MOUSE_SCALE", "min": 0.1, "max": 50, "step": 0.1, "requires": "gh_keyshud_mouse"},
    {"key": "gh_keyshud_align", "type": "select", "default": "9", "label": "#L_GS_X_GH_KEYSHUD_ALIGN", "options": ANCHOR_OPTIONS, "requires": "gh_keyshud"},
    {"key": "gh_keyshud_x", "type": "number", "default": -19, "label": "#L_GS_X_GH_KEYSHUD_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_KEYSHUD_X_HINT", "requires": "gh_keyshud"},
    {"key": "gh_keyshud_y", "type": "number", "default": -55, "label": "#L_GS_X_GH_KEYSHUD_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_KEYSHUD_Y_HINT", "requires": "gh_keyshud"},
  ] },
  { id: "weapon", icon: "⌖", title: "#L_GS_X_SECTION_WEAPON", fields: [
    {"key": "fov_desired", "type": "number", "default": 110, "label": "#L_GS_X_FOV", "min": 75, "max": 150, "step": 1, "unit": "°", "scale": ["#L_GS_X_SCALE_NARROW", "#L_GS_X_SCALE_WIDE"], "hint": "#L_GS_X_FOV_HINT"},
    {"key": "viewmodel_fov", "type": "number", "default": 62, "label": "#L_GS_X_VIEWMODEL_FOV", "min": 10, "max": 100, "step": 1, "unit": "°", "scale": ["#L_GS_X_SCALE_CLOSE", "#L_GS_X_SCALE_FAR"], "hint": "#L_GS_X_VIEWMODEL_FOV_HINT"},
    {"key": "viewmodel_offset_x", "type": "number", "default": 0, "label": "#L_GS_X_VIEWMODEL_OFFSET_X", "min": -5, "max": 5, "step": 0.1, "scale": ["#L_GS_X_SCALE_LEFT", "#L_GS_X_SCALE_RIGHT"]},
    {"key": "viewmodel_offset_y", "type": "number", "default": 0, "label": "#L_GS_X_VIEWMODEL_OFFSET_Y", "min": -4, "max": 4, "step": 0.1, "scale": ["#L_GS_X_SCALE_BACK", "#L_GS_X_SCALE_FORWARD"]},
    {"key": "viewmodel_offset_z", "type": "number", "default": 0, "label": "#L_GS_X_VIEWMODEL_OFFSET_Z", "min": -4, "max": 4, "step": 0.1, "scale": ["#L_GS_X_SCALE_DOWN", "#L_GS_X_SCALE_UP"]},
    {"key": "viewmodel_bobbing", "type": "boolean", "default": true, "label": "#L_GS_X_VIEWMODEL_BOBBING"},
    {"key": "cl_righthand", "type": "boolean", "default": true, "label": "#L_GS_X_CL_RIGHTHAND"},
  ] },
  { id: "meters", icon: "◷", title: "#L_GS_X_SECTION_METERS", fields: [
    {"key": "gh_timer", "type": "boolean", "default": true, "label": "#L_GS_X_GH_TIMER"},
    {"key": "gh_timer_bg", "type": "boolean", "default": true, "label": "#L_GS_X_GH_TIMER_BG", "requires": "gh_timer"},
    {"key": "gh_timer_mirror", "type": "boolean", "default": false, "label": "#L_GS_X_GH_TIMER_MIRROR", "hint": "#L_GS_X_GH_TIMER_MIRROR_HINT", "requires": "gh_timer"},
    {"key": "gh_timer_mirror_port", "type": "number", "default": 16834, "label": "#L_GS_X_GH_TIMER_MIRROR_PORT", "min": 1, "max": 65535, "step": 1, "requires": "gh_timer_mirror"},
    {"key": "gh_timer_mirror_splits", "type": "boolean", "default": true, "label": "#L_GS_X_GH_TIMER_MIRROR_SPLITS", "requires": "gh_timer_mirror"},
    {"key": "gh_timer_mirror_splits_bold", "type": "boolean", "default": true, "label": "#L_GS_X_GH_TIMER_MIRROR_SPLITS_BOLD", "requires": "gh_timer_mirror_splits"},
    {"key": "gh_timer_align", "type": "select", "default": "0", "label": "#L_GS_X_GH_TIMER_ALIGN", "options": ANCHOR_OPTIONS, "requires": "gh_timer"},
    {"key": "gh_timer_x", "type": "number", "default": 0, "label": "#L_GS_X_GH_TIMER_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_TIMER_X_HINT", "requires": "gh_timer"},
    {"key": "gh_timer_y", "type": "number", "default": 0, "label": "#L_GS_X_GH_TIMER_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_TIMER_Y_HINT", "requires": "gh_timer"},
    {"key": "gh_speedmeter", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SPEEDMETER"},
    {"key": "gh_speedmeter_hvel", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SPEEDMETER_HVEL", "requires": "gh_speedmeter"},
    {"key": "gh_speedmeter_bg", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SPEEDMETER_BG", "requires": "gh_speedmeter"},
    {"key": "gh_speedmeter_label", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SPEEDMETER_LABEL", "requires": "gh_speedmeter"},
    {"key": "gh_speedmeter_align", "type": "select", "default": "0", "label": "#L_GS_X_GH_SPEEDMETER_ALIGN", "options": ANCHOR_OPTIONS, "requires": "gh_speedmeter"},
    {"key": "gh_speedmeter_x", "type": "number", "default": 0, "label": "#L_GS_X_GH_SPEEDMETER_X", "min": -640, "max": 640, "step": 1, "hint": "#L_GS_X_GH_SPEEDMETER_X_HINT", "requires": "gh_speedmeter"},
    {"key": "gh_speedmeter_y", "type": "number", "default": 0, "label": "#L_GS_X_GH_SPEEDMETER_Y", "min": -480, "max": 480, "step": 1, "hint": "#L_GS_X_GH_SPEEDMETER_Y_HINT", "requires": "gh_speedmeter"},
    {"key": "cl_showfps", "type": "select", "default": "1", "label": "#L_GS_X_CL_SHOWFPS", "options": [{"value": "0", "label": "#L_GS_X_FPS_MODE_0"}, {"value": "1", "label": "#L_GS_X_FPS_MODE_1"}, {"value": "2", "label": "#L_GS_X_FPS_MODE_2"}]},
    {"key": "cl_showpos", "type": "boolean", "default": true, "label": "#L_GS_X_CL_SHOWPOS"},
    {"key": "gh_showfps_font", "type": "text", "default": "", "label": "#L_GS_X_GH_SHOWFPS_FONT", "hint": "#L_GS_X_GH_SHOWFPS_FONT_HINT", "maxLength": 120, "requires": "cl_showfps"},
    {"key": "gh_showfps_fontsize", "type": "number", "default": 0, "label": "#L_GS_X_GH_SHOWFPS_FONTSIZE", "min": 0, "max": 128, "step": 1, "hint": "#L_GS_X_GH_SHOWFPS_FONTSIZE_HINT", "requires": "cl_showfps"},
    {"key": "gh_showfps_fontweight", "type": "number", "default": 0, "label": "#L_GS_X_GH_SHOWFPS_FONTWEIGHT", "min": 0, "max": 1000, "step": 100, "requires": "cl_showfps"},
    {"key": "gh_showfps_fontoutline", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SHOWFPS_FONTOUTLINE", "requires": "cl_showfps"},
    {"key": "gh_showpos_font", "type": "text", "default": "", "label": "#L_GS_X_GH_SHOWPOS_FONT", "hint": "#L_GS_X_GH_SHOWPOS_FONT_HINT", "maxLength": 120, "requires": "cl_showpos"},
    {"key": "gh_showpos_fontsize", "type": "number", "default": 0, "label": "#L_GS_X_GH_SHOWPOS_FONTSIZE", "min": 0, "max": 128, "step": 1, "hint": "#L_GS_X_GH_SHOWPOS_FONTSIZE_HINT", "requires": "cl_showpos"},
    {"key": "gh_showpos_fontweight", "type": "number", "default": 0, "label": "#L_GS_X_GH_SHOWPOS_FONTWEIGHT", "min": 0, "max": 1000, "step": 100, "requires": "cl_showpos"},
    {"key": "gh_showpos_fontoutline", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SHOWPOS_FONTOUTLINE", "requires": "cl_showpos"},
  ] },
  { id: "ghosts", icon: "♧", title: "#L_GS_X_SECTION_GHOSTS", fields: [
    {"key": "gh_name", "type": "text", "default": "Ghost", "label": "#L_GS_X_GH_NAME", "maxLength": 64},
    {"key": "gh_model", "type": "text", "default": "models/player.mdl", "label": "#L_GS_X_GH_MODEL", "hint": "#L_GS_X_GH_MODEL_HINT", "maxLength": 200},
    {"key": "gh_ghost_color", "type": "color", "default": {"r": 237, "g": 133, "b": 60}, "label": "#L_GS_X_GH_GHOST_COLOR"},
    {"key": "gh_opacity", "type": "number", "default": 75, "label": "#L_GS_X_GH_OPACITY", "min": 0, "max": 255, "step": 1},
    {"key": "gh_opacity_range", "type": "number", "default": 200, "label": "#L_GS_X_GH_OPACITY_RANGE", "min": 0, "max": 5000, "step": 10},
    {"key": "gh_ghost_names", "type": "boolean", "default": true, "label": "#L_GS_X_GH_GHOST_NAMES"},
    {"key": "gh_ghost_names_dist", "type": "number", "default": 2000, "label": "#L_GS_X_GH_GHOST_NAMES_DIST", "min": 0, "max": 20000, "step": 100, "hint": "#L_GS_X_GH_GHOST_NAMES_DIST_HINT", "requires": "gh_ghost_names"},
    {"key": "gh_ghost_names_own_color", "type": "boolean", "default": true, "label": "#L_GS_X_GH_GHOST_NAMES_OWN_COLOR", "requires": "gh_ghost_names"},
    {"key": "gh_draw_trails", "type": "boolean", "default": true, "label": "#L_GS_X_GH_DRAW_TRAILS"},
    {"key": "gh_trail_color", "type": "color", "default": {"r": 237, "g": 133, "b": 60}, "label": "#L_GS_X_GH_TRAIL_COLOR", "requires": "gh_draw_trails"},
    {"key": "gh_trail_length", "type": "number", "default": 5, "label": "#L_GS_X_GH_TRAIL_LENGTH", "min": 0, "max": 60, "step": 1, "requires": "gh_draw_trails"},
    {"key": "gh_show_held_props", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SHOW_HELD_PROPS"},
    {"key": "gh_online_race_spectators", "type": "boolean", "default": false, "label": "#L_GS_X_GH_ONLINE_RACE_SPECTATORS"},
    {"key": "gh_autorecord", "type": "boolean", "default": true, "label": "#L_GS_X_GH_AUTORECORD"},
    {"key": "gh_runcomplete_notify", "type": "boolean", "default": false, "label": "#L_GS_X_GH_RUNCOMPLETE_NOTIFY"},
  ] },
  { id: "chat", icon: "☷", title: "#L_GS_X_SECTION_CHAT", fields: [
    {"key": "gh_chat", "type": "boolean", "default": false, "label": "#L_GS_X_GH_CHAT"},
    {"key": "gh_chat_channel", "type": "text", "default": "", "label": "#L_GS_X_GH_CHAT_CHANNEL", "hint": "#L_GS_X_GH_CHAT_CHANNEL_HINT", "maxLength": 25, "requires": "gh_chat"},
    {"key": "gh_chat_youtube", "type": "boolean", "default": true, "label": "#L_GS_X_GH_CHAT_YOUTUBE", "hint": "#L_GS_X_GH_CHAT_YOUTUBE_HINT", "requires": "gh_chat"},
    {"key": "gh_chat_connect", "type": "text", "default": "", "label": "#L_GS_X_GH_CHAT_CONNECT", "hint": "#L_GS_X_GH_CHAT_CONNECT_HINT", "maxLength": 512, "command": true, "requires": "gh_chat_youtube"},
    {"key": "gh_chat_emotes", "type": "boolean", "default": true, "label": "#L_GS_X_GH_CHAT_EMOTES", "requires": "gh_chat"},
    {"key": "gh_chat_emote_scale", "type": "number", "default": 2, "label": "#L_GS_X_GH_CHAT_EMOTE_SCALE", "min": 0.25, "max": 4, "step": 0.05, "requires": "gh_chat_emotes"},
    {"key": "gh_chat_emoji_scale", "type": "number", "default": 1.15, "label": "#L_GS_X_GH_CHAT_EMOJI_SCALE", "min": 0.25, "max": 4, "step": 0.05, "requires": "gh_chat"},
    {"key": "gh_chat_badges", "type": "boolean", "default": true, "label": "#L_GS_X_GH_CHAT_BADGES", "requires": "gh_chat"},
    {"key": "gh_chat_prefix", "type": "boolean", "default": true, "label": "#L_GS_X_GH_CHAT_PREFIX", "requires": "gh_chat"},
    {"key": "gh_chat_minimal", "type": "boolean", "default": true, "label": "#L_GS_X_GH_CHAT_MINIMAL", "requires": "gh_chat"},
    {"key": "gh_chat_sound", "type": "boolean", "default": true, "label": "#L_GS_X_GH_CHAT_SOUND", "requires": "gh_chat"},
    {"key": "gh_chat_sound_stream", "type": "boolean", "default": false, "label": "#L_GS_X_GH_CHAT_SOUND_STREAM", "requires": "gh_chat_sound"},
  ] },
  { id: "misc", icon: "◇", title: "#L_GS_X_SECTION_MISC", fields: [
    {"key": "gh_hud_dmgflash", "type": "boolean", "default": true, "label": "#L_GS_X_GH_HUD_DMGFLASH"},
    {"key": "gh_hud_radflash", "type": "boolean", "default": true, "label": "#L_GS_X_GH_HUD_RADFLASH"},
    {"key": "gh_hud_color_affects_damage", "type": "boolean", "default": false, "label": "#L_GS_X_GH_HUD_COLOR_AFFECTS_DAMAGE"},
    {"key": "gh_autosave", "type": "boolean", "default": true, "label": "#L_GS_X_GH_AUTOSAVE"},
    {"key": "gh_sprays", "type": "boolean", "default": true, "label": "#L_GS_X_GH_SPRAYS"},
    {"key": "gh_spray_file", "type": "text", "default": "", "label": "#L_GS_X_GH_SPRAY_FILE", "hint": "#L_GS_X_GH_SPRAY_FILE_HINT", "maxLength": 512},
    {"key": "gh_spray_scale", "type": "number", "default": 0.25, "label": "#L_GS_X_GH_SPRAY_SCALE", "min": 0.05, "max": 2, "step": 0.05, "requires": "gh_sprays"},
    {"key": "gh_spray_max", "type": "number", "default": 10, "label": "#L_GS_X_GH_SPRAY_MAX", "min": 1, "max": 32, "step": 1, "requires": "gh_sprays"},
  ] },
];
const FIELDS = SECTIONS.reduce((all, section) => all.concat(section.fields), []);
const BY_KEY = Object.create(null);
FIELDS.forEach((field) => { BY_KEY[field.key] = field; });
const own = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
// A setting with "requires" does nothing in the game while that setting is off
// or empty (checked in Ghosting Mod Renewed 3.9: the HUD font scale, weight
// and outline, for one, change nothing without gh_hud_font). Requirements
// chain, so a setting is only live when every setting above it is.
const isOn = (value) => !!value && value !== "0";
function available(field, values) {
  for (let key = field.requires; key; key = BY_KEY[key].requires) {
    if (!isOn(own(values, key) ? values[key] : BY_KEY[key].default)) return false;
  }
  return true;
}
const MANAGED_SPRAY_PATH = /^materials\/vgui\/logos\/hlsr\/[a-f0-9]{64}\.(png|bmp|tga)$/;

function cloneDefault(field) {
  return field.type === "color" ? Object.assign({}, field.default) : field.default;
}

function defaults() {
  const result = {};
  FIELDS.forEach((field) => { result[field.key] = cloneDefault(field); });
  return result;
}

// Only reset fields the launcher already manages. Opening the page, saving a
// crosshair, or resetting one section must not claim every mod setting.
function resetExtras(input) {
  const clean = sanitizeExtras(input);
  const result = {};
  Object.keys(clean).forEach((key) => { result[key] = cloneDefault(BY_KEY[key]); });
  return result;
}

function youtubeUrl(value) {
  if (value === "") return "";
  try {
    const url = new URL(value);
    if (url.protocol !== "https:" || url.username || url.password || url.port) return undefined;
    let id;
    if (url.hostname === "youtu.be") id = url.pathname.slice(1);
    else if (["youtube.com", "www.youtube.com", "m.youtube.com"].includes(url.hostname)) {
      if (["/watch", "/live_chat", "/live_chat_replay"].includes(url.pathname)) {
        id = url.searchParams.get("v");
      } else {
        const match = /^\/live\/([A-Za-z0-9_-]+)\/?$/.exec(url.pathname);
        id = match && match[1];
      }
    }
    return typeof id === "string" && /^[A-Za-z0-9_-]{11}$/.test(id)
      ? `https://www.youtube.com/watch?v=${id}` : undefined;
  } catch (error) {
    return undefined;
  }
}

function sanitizeValue(field, value) {
  if (field.type === "boolean") {
    if (value === true || value === 1 || value === "1") return true;
    if (value === false || value === 0 || value === "0") return false;
    return undefined;
  }
  if (field.type === "number") {
    if (typeof value !== "number" && typeof value !== "string") return undefined;
    if (typeof value === "string" && !value.trim()) return undefined;
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) return undefined;
    const bounded = Math.max(field.min, Math.min(field.max, numeric));
    return field.step >= 1 ? Math.round(bounded) : Number(bounded.toFixed(6));
  }
  if (field.type === "select") {
    if (typeof value !== "string" && typeof value !== "number") return undefined;
    return field.options.some((option) => option.value === String(value)) ? String(value) : undefined;
  }
  if (field.type === "color") {
    let channels;
    if (typeof value === "string") {
      const hex = /^#([a-f\d]{6})$/i.exec(value);
      channels = hex ? [0, 2, 4].map((offset) => parseInt(hex[1].slice(offset, offset + 2), 16))
        : value.trim().split(/\s+/).map(Number);
    } else if (value && typeof value === "object" && !Array.isArray(value)) {
      channels = [value.r, value.g, value.b];
    }
    if (!channels || channels.length !== 3 || !channels.every(Number.isFinite)) return undefined;
    const rgb = channels.map((channel) => Math.max(0, Math.min(255, Math.round(channel))));
    return { r: rgb[0], g: rgb[1], b: rgb[2] };
  }
  if (typeof value !== "string" || value.length > field.maxLength) return undefined;
  // Values are quoted in Source cfg files. Refuse separators and quote/control
  // characters instead of attempting shell-style escaping that Source ignores.
  // eslint-disable-next-line no-control-regex
  if (/[";\x00-\x1f\x7f-\x9f\u2028\u2029]/.test(value)) return undefined;
  const text = value.trim().replace(/\\/g, "/");
  // gh_spray_set открывает путь как есть: сетевая папка дала бы чужому серверу соединение с компьютера игрока.
  if (field.key === "gh_spray_file" && (text.startsWith("//") || text.split("/").includes(".."))) return undefined;
  if (field.key === "gh_spray_file" && text.startsWith("ghostingmod/") && MANAGED_SPRAY_PATH.test(text.slice(12))) {
    return text.slice(12);
  }
  if (field.key === "gh_chat_channel") return /^[a-z0-9_]{0,25}$/i.test(text) ? text.toLowerCase() : undefined;
  if (field.key === "gh_chat_connect") return youtubeUrl(text);
  return text;
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

function serializeExtras(input) {
  const clean = sanitizeExtras(input);
  const connections = [];
  const lines = FIELDS.filter((field) => own(clean, field.key)).map((field) => {
    const value = clean[field.key];
    if (field.key === "gh_spray_file") {
      // Unlike HUD fonts, gh_spray_set opens the path directly from the game
      // process working directory (the install root), not Source's mod search path.
      const file = MANAGED_SPRAY_PATH.test(value) ? `ghostingmod/${value}` : value;
      return file ? `gh_spray_set "${file}"` : "gh_spray_clear";
    }
    if (field.key === "gh_chat_connect") {
      if (!value) connections.push("gh_chat_disconnect youtube");
      else if (clean.gh_chat !== false && clean.gh_chat_youtube !== false) connections.push(`gh_chat_connect "${value}"`);
      // Remember the selected URL without reconnecting an explicitly disabled
      // reader. This comment is parsed only as data by importExtras.
      return `// HLSR_YOUTUBE_URL "${value}"`;
    }
    // The view's field of view has three names in Source; the game reads
    // whichever it reads at the time, so all three get the same value.
    if (field.key === "fov_desired") return `default_fov "${value}"\nfov_desired "${value}"\nfov "${value}"`;
    if (field.key === "gh_chat_channel") {
      if (!value) connections.push("gh_chat_disconnect twitch");
      else if (clean.gh_chat !== false) connections.push(`gh_chat_connect "${value}"`);
    }
    const serialized = field.type === "boolean" ? (value ? "1" : "0")
      : field.type === "color" ? `${value.r} ${value.g} ${value.b}` : String(value);
    return `${field.key} "${serialized}"`;
  });
  return lines.concat(connections).join("\n");
}

function importExtras(text) {
  const parsed = {};
  if (typeof text !== "string") return parsed;
  text.split(/\r?\n/).forEach((line) => {
    const metadata = /^\s*\/\/ HLSR_YOUTUBE_URL "([^"\r\n]*)"\s*$/.exec(line);
    if (metadata) {
      const value = sanitizeValue(BY_KEY.gh_chat_connect, metadata[1]);
      if (value !== undefined) parsed.gh_chat_connect = value;
      return;
    }
    const match = /^\s*([a-z0-9_.]+)(?:\s+(?:"([^"\r\n]*)"|([^"\r\n]*?)))?\s*(?:\/\/.*)?$/i.exec(line);
    if (!match) return;
    const command = match[1].toLowerCase();
    const value = match[2] !== undefined ? match[2] : (match[3] || "").trim();
    let key = command;
    let raw = value;
    if (command === "gh_spray_set") key = "gh_spray_file";
    if (command === "gh_spray_clear" && !value) { key = "gh_spray_file"; raw = ""; }
    if (command === "gh_chat_disconnect" && /^(youtube|all)$/i.test(value)) { key = "gh_chat_connect"; raw = ""; }
    if (!BY_KEY[key]) return;
    const clean = sanitizeValue(BY_KEY[key], raw);
    if (clean !== undefined) parsed[key] = clean;
  });
  return parsed;
}

const isManagedSpray = (value) => MANAGED_SPRAY_PATH.test(value);

module.exports = { SECTIONS, FIELDS, defaults, available, resetExtras, sanitizeExtras, serializeExtras, importExtras, isManagedSpray };
