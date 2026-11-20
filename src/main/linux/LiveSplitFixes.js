"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const FIXES_DIRECTORY = "livesplit-fixes";

// Исправленные под Wine компоненты LiveSplit. Файл заменяется, только если на
// диске ровно та сборка, из которой сделано исправление: компонент, который
// пользователь обновил сам, лаунчер не трогает.
const LIVESPLIT_FIXES = [
  {
    // SourceSplit 3.3.4.6. Если LiveSplit запущен раньше игры, wine-mono
    // бросает ArgumentOutOfRangeException из Process.MainModule, пока у только
    // что созданного процесса пуст список модулей, и SourceSplit показывает
    // окно ошибки (в Windows там Win32Exception, её он пропускает). В
    // исправлении этот вызов в TryGetGameProcess возвращает null, и SourceSplit
    // повторяет попытку. Патч: scripts/sourcesplit-patch/patch.cs.
    target: "Components/LiveSplit.SourceSplit.dll",
    source: "LiveSplit.SourceSplit.dll",
    original: "f8461b811571dbede634c9bd048d6fc05a64d5e949a52e2ec339bc27149871e8",
    fixed: "a02c1a2b545b0b17b889a795f7fd5ddc44807398d9ad4c683292aa2a558a20f1",
  },
];

function sha256Of(fsModule, file) {
  return crypto.createHash("sha256").update(fsModule.readFileSync(file)).digest("hex");
}

// Возвращает список заменённых файлов (пути относительно папки LiveSplit).
function applyLiveSplitFixes(root, options = {}) {
  const fsModule = options.fs || fs;
  const fixes = options.fixes || LIVESPLIT_FIXES;
  const applied = [];
  for (const fix of fixes) {
    const target = path.join(root, fix.target);
    let current;
    try { current = sha256Of(fsModule, target); }
    catch (error) { continue; }
    if (current !== fix.original) continue;
    const source = path.join(options.resourcesPath || process.resourcesPath, FIXES_DIRECTORY, fix.source);
    if (sha256Of(fsModule, source) !== fix.fixed)
      throw new Error(`Исправление LiveSplit повреждено: ${fix.source}`);
    const temporary = `${target}.${process.pid}.tmp`;
    fsModule.copyFileSync(source, temporary);
    fsModule.renameSync(temporary, target);
    applied.push(fix.target);
  }
  return applied;
}

module.exports = { applyLiveSplitFixes, LIVESPLIT_FIXES, FIXES_DIRECTORY };
