"use strict";

// Чего нет у системы, на которой идут самотесты. Значение — причина для { skip }
// или t.skip(), false — возможность есть. Пробуется только Windows: на Linux всё
// это есть, там значения всегда false и тесты идут как раньше.
const fs = require("fs");
const os = require("os");
const path = require("path");

const windows = process.platform === "win32";

function fails(probe) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-caps-"));
  try { probe(dir); return false; } catch (error) { return true; } finally { fs.rmSync(dir, { recursive: true, force: true }); }
}

module.exports = {
  // Без режима разработчика или прав администратора symlinkSync даёт EPERM (junction права не требует).
  NO_SYMLINKS: windows && fails((dir) => fs.symlinkSync(path.join(dir, "target"), path.join(dir, "link")))
    && "нет права на символические ссылки (Windows без режима разработчика)",
  // NTFS по умолчанию не различает регистр: games и Games — одна папка.
  NO_CASE_SENSITIVITY: windows && fails((dir) => {
    fs.writeFileSync(path.join(dir, "a"), "");
    if (fs.existsSync(path.join(dir, "A"))) throw new Error("регистр не различается");
  }) && "файловая система не различает регистр имён",
  NO_EXEC_BIT: windows && "у файлов Windows нет бита исполнения",
  NO_SHELL_SCRIPTS: windows && "Windows не запускает скрипты #!/bin/sh (заглушки AppImage и утилит)",
  NO_GETUID: typeof process.getuid !== "function" && "нет process.getuid",
  NO_POSIX_PATHS: path.sep !== "/" && "код Linux собирает пути через path, на Windows выходят пути Windows",
  // Код под тестом сам смотрит на process.platform — на другой системе он идёт другой веткой.
  NOT_LINUX: process.platform !== "linux" && "код под тестом выбирает ветку Linux по process.platform",
  // tar из Git Bash (GNU) принимает «C:» за адрес сервера; системный bsdtar Windows — нет.
  TAR: windows ? path.join(process.env.SystemRoot || "C:\\Windows", "System32", "tar.exe") : "tar",
};
