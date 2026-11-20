import settings from "@/utils/settingsStore";

// Масштаб интерфейса (вкладка «Интерфейс»). Лежит в settings.json: main
// читает его до создания окна (src/main/CreateWindow.js).
const KEY = "uiScale";
const SCALES = [0.75, 1, 1.25, 1.5];
const DEFAULT_SCALE = 1;

function normalize(value) {
  const scale = Number(value);
  return SCALES.includes(scale) ? scale : DEFAULT_SCALE;
}

function read() {
  const config = settings.get("config") || {};
  return normalize(config[KEY]);
}

function write(scale) {
  const config = Object.assign({}, settings.get("config") || {});
  config[KEY] = normalize(scale);
  return settings.set("config", config);
}

// Окно меняет размер сразу. Возвращает масштаб, который поместился на экран.
async function apply(scale) {
  try {
    const result = await window.hlsrElectron.ipcRenderer.invoke("ui-scale-apply", normalize(scale));
    return result && result.status === "ok" ? result.scale : null;
  } catch (error) {
    return null;
  }
}

export default { read, write, apply, normalize, SCALES, DEFAULT_SCALE, KEY };
