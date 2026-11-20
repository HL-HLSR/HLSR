"use strict";

// Windows-Electron под Wine: байткод Windows-сборки, собранной на Linux,
// компилирует и проверяет Electron той же платформы, что будет его исполнять
// (scripts/compile-main-bytecode.js). Wine — системный, с префиксом по
// умолчанию: тот же, через который electron-builder правит ресурсы exe.
//
// Под Wine Windows-Node не может писать в unix-каналы (open EBADF), а
// bytenode и проверка байткода читают вывод Electron через каналы. Поэтому
// Electron запускается обёрткой: его вывод идёт в файл, который обёртка
// затем пересылает в свой stderr, а код выхода передаётся как есть.
const fs = require("fs");
const os = require("os");
const path = require("path");

const ARCH = "x64";

function cacheDir(version) {
  return path.join(os.homedir(), ".cache", "hlsr", `electron-v${version}-win32-${ARCH}`);
}

function wrapperSource(exe) {
  return [
    "#!/bin/sh",
    "# Создано scripts/lib/wine-electron.js: Windows-Electron под Wine с выводом через файл.",
    'log=$(mktemp "${TMPDIR:-/tmp}/hlsr-wine-electron.XXXXXX") || exit 1',
    `WINEDEBUG=-all wine ${JSON.stringify(exe)} "$@" </dev/null >"$log" 2>&1`,
    "code=$?",
    'cat "$log" >&2',
    'rm -f "$log"',
    'exit "$code"',
    "",
  ].join("\n");
}

// Путь к обёртке, которую можно передать как electronPath. Electron
// скачивается через @electron/get (проверка SHA-256, общий кеш с
// electron-builder) и распаковывается один раз на версию.
async function electronPath(version) {
  const electronVersion = version || require("electron/package.json").version;
  const dir = cacheDir(electronVersion);
  const exe = path.join(dir, "electron.exe");
  if (!fs.existsSync(exe)) {
    const { downloadArtifact } = require("@electron/get");
    const zip = await downloadArtifact({ version: electronVersion, platform: "win32", arch: ARCH, artifactName: "electron" });
    const partial = `${dir}.partial`;
    fs.rmSync(partial, { recursive: true, force: true });
    const unpacked = require("child_process").spawnSync(require("7zip-bin").path7za, ["x", "-y", `-o${partial}`, zip], { stdio: "ignore" });
    if (unpacked.status !== 0) throw new Error(`Не удалось распаковать ${zip}`);
    fs.rmSync(dir, { recursive: true, force: true });
    fs.renameSync(partial, dir);
  }
  const wrapper = path.join(dir, "wine-electron.sh");
  const source = wrapperSource(exe);
  if (!fs.existsSync(wrapper) || fs.readFileSync(wrapper, "utf8") !== source) fs.writeFileSync(wrapper, source, { mode: 0o755 });
  return wrapper;
}

module.exports = { electronPath, wrapperSource, cacheDir };
