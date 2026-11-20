"use strict";

const path = require("path");
const { spawnSync, execFile } = require("child_process");

const { systemRoot, powershellPath } = require("../windows-tools");

// hl.exe из WON собран без NX_COMPAT, а hw.dll расшифровывает движок в
// неисполняемую память. При DEP в режиме OptOut такой hl.exe падает на
// первом же переходе в движок. Слой совместимости DisableNXShowUI снимает
// DEP с одного exe, это та же запись, что делает окно «Предотвращение
// выполнения данных» в Windows. В HKCU права администратора не нужны.
const COMPAT_LAYERS = "HKCU\\Software\\Microsoft\\Windows NT\\CurrentVersion\\AppCompatFlags\\Layers";
const LAYER = "DisableNXShowUI";

// Win32_OperatingSystem.DataExecutionPrevention_SupportPolicy.
const POLICY = Object.freeze({ 0: "AlwaysOff", 1: "AlwaysOn", 2: "OptIn", 3: "OptOut" });

// STATUS_ACCESS_VIOLATION: так завершается процесс, остановленный DEP.
const ACCESS_VIOLATION = 0xc0000005;

function regPath(env) {
  return path.win32.join(systemRoot(env), "System32", "reg.exe");
}

function defaultRun(args) {
  return spawnSync(regPath(), args, { windowsHide: true, encoding: "utf8", timeout: 10000 });
}

// Данные значения из вывода `reg query`: «    <имя>    REG_SZ    <данные>».
function layersOf(stdout, exe) {
  const target = String(exe).toLowerCase();
  for (const line of String(stdout || "").split(/\r?\n/)) {
    const match = /^\s*(.+?)\s{2,}REG_SZ\s{2,}(.*)$/.exec(line);
    if (match && match[1].trim().toLowerCase() === target) return match[2].trim();
  }
  return null;
}

// Слой дописывается к уже стоящим флагам (RUNASADMIN, режим совместимости),
// чтобы не снять то, что пользователь выставил в свойствах exe.
function mergeLayers(current) {
  const tokens = String(current || "").split(/\s+/).filter(Boolean);
  if (tokens.some((token) => token.toLowerCase() === LAYER.toLowerCase())) return null;
  const marker = tokens[0] === "~" ? tokens.shift() : "~";
  return [marker, ...tokens, LAYER].join(" ");
}

// Ставит слой для exe. Возвращает "present", "added" или "failed".
function ensureExemption(exe, options) {
  const opts = options || {};
  const run = opts.run || defaultRun;
  const logger = opts.logger || { info() {}, warn() {} };

  let current = null;
  try {
    const query = run(["query", COMPAT_LAYERS, "/v", exe]);
    if (query && query.status === 0) current = layersOf(query.stdout, exe);
  } catch (error) {
    logger.warn(`[dep] слой совместимости не прочитан: ${error.message}`);
  }

  const next = mergeLayers(current);
  if (next === null) return "present";

  try {
    const result = run(["add", COMPAT_LAYERS, "/v", exe, "/t", "REG_SZ", "/d", next, "/f"]);
    if (result && result.status === 0) {
      logger.info(`[dep] для ${exe} поставлен слой ${LAYER}${current ? ` (было: ${current})` : ""}`);
      return "added";
    }
    const reason = result && result.error ? result.error.message : String((result && result.stderr) || "").trim();
    logger.warn(`[dep] слой ${LAYER} не записан: ${reason || `код ${result ? result.status : "?"}`}`);
  } catch (error) {
    logger.warn(`[dep] слой ${LAYER} не записан: ${error.message}`);
  }
  return "failed";
}

function parsePolicy(stdout) {
  const value = Number.parseInt(String(stdout || "").trim(), 10);
  return Object.prototype.hasOwnProperty.call(POLICY, value) ? value : null;
}

// Режим DEP системы. null, если узнать не удалось.
function queryPolicy(options) {
  const opts = options || {};
  const execFileFn = opts.execFile || execFile;
  return new Promise((resolve) => {
    try {
      execFileFn(
        powershellPath(),
        ["-NoProfile", "-NonInteractive", "-Command",
          "(Get-CimInstance Win32_OperatingSystem).DataExecutionPrevention_SupportPolicy"],
        { windowsHide: true, timeout: 15000 },
        (error, stdout) => resolve(error ? null : parsePolicy(stdout))
      );
    } catch (error) {
      resolve(null);
    }
  });
}

// Подсказка к падению HL1: при AlwaysOn исключения для программ не действуют,
// при OptOut — если слой не записался. hl.exe запускает инжектор, поэтому
// код выхода чаще неизвестен (null); известный и другой — не DEP.
function crashHint(state, exitCode) {
  if (!state) return null;
  if (exitCode != null && Number(exitCode) >>> 0 !== ACCESS_VIOLATION) return null;
  if (state.policy === 1) return "DEP_ALWAYS_ON";
  if (state.policy === 3 && state.exemption === "failed") return "DEP_EXEMPTION_FAILED";
  return null;
}

module.exports = {
  COMPAT_LAYERS,
  LAYER,
  POLICY,
  ACCESS_VIOLATION,
  layersOf,
  mergeLayers,
  ensureExemption,
  parsePolicy,
  queryPolicy,
  crashHint,
};
