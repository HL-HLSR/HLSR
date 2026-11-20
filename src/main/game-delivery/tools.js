"use strict";

const livesplitManifest = require("../../../config/tools/livesplit.manifest.json");
const ffmpegManifest = require("../../../config/tools/ffmpeg.manifest.json");
const bxtrsManifest = require("../../../config/tools/bxt-rs.manifest.json");

const LIVESPLIT = "livesplit";
const FFMPEG = "ffmpeg";
const BXTRS = "bxt-rs";

const REGISTRY = {
  [LIVESPLIT]: livesplitManifest,
};

// Зависимости других инструментов: ставятся тем же ToolService, но на панели
// инструментов отдельной карточкой не показываются (FFmpeg и bxt-rs — для «Демо»).
const DEPENDENCIES = {
  [FFMPEG]: ffmpegManifest,
  [BXTRS]: bxtrsManifest,
};

const PRESENTATION = {
  [LIVESPLIT]: {
    shortName: "LiveSplit",
    title: "LiveSplit",
    subtitle: "#L_TOOL_LIVESPLIT_SUB",
    description: "#L_TOOL_LIVESPLIT_DESC",
    icon: "fas fa-stopwatch",
    accent: "#3fa7ff",
    accentSoft: "rgba(63, 167, 255, 0.16)",
    help: "https://youtu.be/l-lZ1yr_Mlc?t=316",
  },
};

function own(table, key) {
  return Object.prototype.hasOwnProperty.call(table, key) ? table[key] : null;
}

function manifestOf(toolId) {
  return own(REGISTRY, String(toolId));
}

function presentationOf(toolId) {
  return own(PRESENTATION, String(toolId));
}

function ids() {
  return Object.keys(REGISTRY);
}

module.exports = {
  LIVESPLIT,
  FFMPEG,
  BXTRS,
  REGISTRY,
  DEPENDENCIES,
  PRESENTATION,
  manifestOf,
  presentationOf,
  ids,
};
