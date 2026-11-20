"use strict";

const { execFileSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const { powershellPath } = require("../windows-tools");

const START_TOLERANCE_MS = 5000;

// Windows: данные о процессах отдаёт PowerShell через WMI (Get-CimInstance).
// На части машин WMI сломан или отключён, а каталога PowerShell нет в PATH —
// тогда PowerShell берётся по полному пути, вместо WMI спрашивается
// Get-Process (тот же путь exe, без WMI), а если не запустился и PowerShell,
// остаётся tasklist: он отдаёт только имя образа, без пути.
const PROCESS_INFO =
  "function HlsrCim($p) { [pscustomobject]@{" +
  " pid = [int]$p.ProcessId;" +
  " executablePath = $p.ExecutablePath;" +
  " name = $p.Name;" +
  " startedAt = if ($p.CreationDate) { $p.CreationDate.ToUniversalTime().ToString('o') } else { $null };" +
  " source = 'cim' } };" +
  "function HlsrProc($p) { $exe = $null; $start = $null;" +
  " try { $exe = $p.Path } catch {}; try { $start = $p.StartTime.ToUniversalTime().ToString('o') } catch {};" +
  " [pscustomobject]@{ pid = [int]$p.Id; executablePath = $exe; name = $p.ProcessName + '.exe'; startedAt = $start; source = 'process' } };";

const POWERSHELL_QUERY =
  "$ErrorActionPreference='Stop';" +
  PROCESS_INFO +
  "$id = [int]$env:HLSR_PID;" +
  "try { $out = @(Get-CimInstance Win32_Process -Filter \"ProcessId=$id\" | ForEach-Object { HlsrCim $_ }) }" +
  " catch { $out = @(Get-Process -Id $id -ErrorAction SilentlyContinue | ForEach-Object { HlsrProc $_ }) };" +
  "if ($out.Count -eq 0) { '' } else { $out[0] | ConvertTo-Json -Compress }";

const POWERSHELL_BY_IMAGE =
  "$ErrorActionPreference='Stop';" +
  PROCESS_INFO +
  "try { $out = @(Get-CimInstance Win32_Process -Filter \"Name='$env:HLSR_IMAGE'\" | ForEach-Object { HlsrCim $_ }) }" +
  " catch { $base = [IO.Path]::GetFileNameWithoutExtension($env:HLSR_IMAGE);" +
  "  $out = @(Get-Process -Name $base -ErrorAction SilentlyContinue | ForEach-Object { HlsrProc $_ }) };" +
  "if ($out.Count -eq 0) { '[]' } else { ,$out | ConvertTo-Json -Compress -Depth 3 }";

function isAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;

  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

function systemTool(env, existsSync, parts, fallback) {
  const root = (env && (env.SystemRoot || env.windir)) || "C:\\Windows";
  const full = path.win32.join(root, "System32", ...parts);
  try {
    if (existsSync(full)) return full;
  } catch (error) {
    // нет доступа к каталогу — пусть ищет PATH
  }
  return fallback;
}

function shortError(error) {
  const text = String((error && (error.stderr || error.message)) || error || "").replace(/\s+/g, " ").trim();
  return text.length > 200 ? `${text.slice(0, 200)}…` : text;
}

function toProcessList(data) {
  if (Array.isArray(data)) return data;
  if (!data || typeof data !== "object") return [];
  if (Array.isArray(data.value)) return data.value;
  return [data];
}

function normalizeItem(item, source) {
  return {
    pid: item.pid,
    executablePath: item.executablePath || null,
    name: item.name || null,
    startedAt: item.startedAt || null,
    source: item.source || source,
  };
}

// tasklist /FO CSV /NH: "steam.exe","7440","Console","1","123 456 K".
// Строка «нет задач» не в кавычках и локализована — она просто пропускается.
function parseTasklist(output) {
  const found = [];
  for (const line of String(output || "").split(/\r?\n/)) {
    const cells = line.trim().match(/^"([^"]*)","(\d+)"/);
    if (!cells) continue;
    const pid = Number(cells[2]);
    if (Number.isInteger(pid) && pid > 0)
      found.push({ pid, executablePath: null, name: cells[1], startedAt: null, source: "tasklist" });
  }
  return found;
}

function createWindowsInspector(options) {
  const opts = options || {};
  const exec = opts.execFileSync || execFileSync;
  const env = opts.env || process.env;
  const existsSync = opts.existsSync || fs.existsSync;
  let lastFailure = null;

  const powershell = () => powershellPath(env);
  const tasklist = () => systemTool(env, existsSync, ["tasklist.exe"], "tasklist.exe");

  // null — PowerShell не отработал (тогда спрашиваем tasklist), иначе разобранный JSON.
  function runPowerShell(script, extraEnv) {
    let output;
    try {
      output = exec(
        powershell(),
        ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script],
        {
          encoding: "utf8",
          windowsHide: true,
          timeout: 10000,
          env: Object.assign({}, env, extraEnv),
        }
      );
    } catch (error) {
      lastFailure = `powershell: ${shortError(error)}`;
      return null;
    }

    const trimmed = String(output || "").trim();
    if (!trimmed) return { data: null };
    try {
      return { data: JSON.parse(trimmed) };
    } catch (error) {
      lastFailure = `powershell: непонятный ответ: ${shortError(trimmed)}`;
      return null;
    }
  }

  function runTasklist(filter) {
    try {
      const output = exec(tasklist(), ["/FO", "CSV", "/NH", "/FI", filter], {
        encoding: "utf8",
        windowsHide: true,
        timeout: 10000,
      });
      return parseTasklist(output);
    } catch (error) {
      lastFailure = `${lastFailure ? `${lastFailure}; ` : ""}tasklist: ${shortError(error)}`;
      return [];
    }
  }

  function inspect(pid) {
    const result = runPowerShell(POWERSHELL_QUERY, { HLSR_PID: String(pid) });
    if (result) {
      const data = result.data;
      if (!data || !data.pid) return null;
      return normalizeItem(data, "cim");
    }

    return runTasklist(`PID eq ${pid}`).find((item) => item.pid === pid) || null;
  }

  function findByImageName(imageName) {
    const result = runPowerShell(POWERSHELL_BY_IMAGE, { HLSR_IMAGE: imageName });
    const list = result
      ? toProcessList(result.data).map((item) => item && normalizeItem(item, "cim"))
      : runTasklist(`IMAGENAME eq ${imageName}`);

    return list.filter((item) => item && Number.isInteger(item.pid) && item.pid > 0);
  }

  return {
    inspect,
    findByImageName,
    lastFailure: () => lastFailure,
  };
}

const windowsInspector = createWindowsInspector();

// Linux: путь и имя процесса читаются из /proc без запуска внешних команд.
function inspectPosix(pid) {
  if (!isAlive(pid)) return null;

  try {
    const root = `/proc/${pid}`;
    const executablePath = fs.readlinkSync(path.join(root, "exe"));
    const name = fs.readFileSync(path.join(root, "comm"), "utf8").trim();
    return { pid, executablePath, name, startedAt: null };
  } catch (error) {
    return null;
  }
}

function inspect(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return null;
  return process.platform === "win32" ? windowsInspector.inspect(pid) : inspectPosix(pid);
}

const IMAGE_NAME = /^[A-Za-z0-9._-]{1,64}$/;

function findByImageNamePosix(imageName) {
  let names;
  try {
    names = fs.readdirSync("/proc");
  } catch (error) {
    return [];
  }

  const found = [];
  for (const name of names) {
    if (!/^\d+$/.test(name)) continue;
    const item = inspectPosix(Number(name));
    if (item && item.name === imageName) found.push(item);
  }

  return found;
}

function findByImageName(imageName) {
  if (typeof imageName !== "string" || !IMAGE_NAME.test(imageName)) return [];
  if (process.platform !== "win32") return findByImageNamePosix(imageName);
  return windowsInspector.findByImageName(imageName);
}

// Почему последний запрос к системе не дал данных (только Windows) — для логов.
function lastFailure() {
  return process.platform === "win32" ? windowsInspector.lastFailure() : null;
}

function samePath(left, right) {
  if (!left || !right) return false;
  if (process.platform !== "win32") return path.resolve(left) === path.resolve(right);
  return path.resolve(left).toLowerCase() === path.resolve(right).toLowerCase();
}

// Без обоих времён старта PID не опознать: его мог получить чужой процесс.
function sameStart(expected, actual) {
  const a = Date.parse(expected);
  const b = Date.parse(actual);

  return !Number.isNaN(a) && !Number.isNaN(b) && Math.abs(a - b) <= START_TOLERANCE_MS;
}

module.exports = {
  START_TOLERANCE_MS,
  isAlive,
  inspect,
  findByImageName,
  lastFailure,
  toProcessList,
  parseTasklist,
  createWindowsInspector,
  samePath,
  sameStart,
};
