"use strict";

const path = require("path");

// Системные программы Windows вызываются по полному пути: каталог
// WindowsPowerShell бывает выпадает из PATH (сторонние установщики, «чистка»
// переменных), и тогда powershell.exe по имени не находится вовсе.
function systemRoot(env) {
  const source = env || process.env;
  return source.SystemRoot || source.WINDIR || "C:\\Windows";
}

function powershellPath(env) {
  return path.win32.join(systemRoot(env), "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
}

module.exports = { systemRoot, powershellPath };
