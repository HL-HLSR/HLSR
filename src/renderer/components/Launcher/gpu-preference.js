import settings from "@/utils/settingsStore";

const KEY = "gpuAcceleration";
const DEFAULT_ENABLED = true;

function read() {
  const config = settings.get("config") || {};
  return typeof config[KEY] === "boolean" ? config[KEY] : DEFAULT_ENABLED;
}

function write(enabled) {
  const config = Object.assign({}, settings.get("config") || {});
  config[KEY] = enabled === true;
  return settings.set("config", config);
}

export default { read, write, KEY, DEFAULT_ENABLED };
