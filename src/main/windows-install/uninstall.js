"use strict";

// Удаление HLSR в Windows. Запускается самим HLSR.exe с --hlsr-uninstall
// (так записано в «Программах и компонентах») и открывает то же окно, что
// и установщик. С галочкой «Также удалить игры» удаление полное: игры,
// папка HLSR и данные в %APPDATA% и %LOCALAPPDATA% (настройки, вход,
// журналы). Без неё данные остаются, и переустановка сохранит вход.
//
// Каталог программы и данные удаляются уже после выхода: работающий
// HLSR.exe и его Chromium держат файлы. Перед этим проверяется, что каталог
// наш, — так же, как это делал прежний деинсталлятор: путь не системный, на
// пути нет junction и symlink, рядом лежит HLSR.exe, а маркер владения байт
// в байт совпадает с installId из реестра.
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const fs = require("../launcher-update/fs-compat");
const layout = require("./layout");
const fileList = require("./file-list");
const { treeSize } = require("../uninstall/launcher-uninstall");
const libraryCleanup = require("../uninstall/library-cleanup");
const updaterMode = require("../launcher-update/updater-mode");
const { UPDATER_EXE } = require("../launcher-update/updater-launch");
const { APPLICATION_ID } = require("../discord-rpc/constants");

// Протокол, который регистрирует Discord Rich Presence (discord-rpc →
// app.setAsDefaultProtocolClient): ключ HKCU\Software\Classes\discord-<id>.
const DISCORD_PROTOCOL = APPLICATION_ID ? `discord-${APPLICATION_ID}` : null;
// Копии апдейтера в TEMP (updater-launch.js uniqueDir).
const UPDATER_STAGE_RE = /^HLSR-Updater-[a-z0-9]+-[a-f0-9]{12}$/;

function exists(target) {
  try {
    fs.lstatSync(target);
    return true;
  } catch (_) {
    return false;
  }
}

// Нечитаемый маркер (занят, нет прав) считается чужим.
const UNREADABLE = "\0нечитаемый маркер";

// null — маркера нет, UNREADABLE — есть, но не читается.
// Каталог в полной форме: регистр и короткие имена 8.3 (PROGRA~1) раскрыты.
// Нет пути или он не читается — как есть. Только для сравнения: проверка
// ссылок на пути (ownershipProblem) идёт по исходному пути.
function realDir(dir) {
  try {
    return fs.realpathSync.native(path.resolve(String(dir)));
  } catch (_) {
    return path.resolve(String(dir));
  }
}

function sameDir(a, b) {
  return layout.isSameDir(realDir(a), realDir(b));
}

function readMarker(dir) {
  try {
    return fs.readFileSync(path.join(dir, layout.OWNER_MARKER), "utf8").replace(/[\r\n]+$/, "");
  } catch (error) {
    return error.code === "ENOENT" ? null : UNREADABLE;
  }
}

// Свой маркер владения или null, если его нет или он не читается.
function ownMarkerOf(installDir) {
  const marker = readMarker(installDir);
  return marker === UNREADABLE ? null : marker;
}

// Другая установка HLSR в каталоге (маркер владения не этой установки) на
// глубине до трёх уровней, без перехода по ссылкам; глубже чужой каталог
// бережёт library-cleanup. Свой каталог программы пропускается.
function otherInstall(root, installDir) {
  const own = ownMarkerOf(installDir);
  const walk = (dir, level) => {
    if (layout.isSameDir(dir, installDir)) return null;
    const marker = readMarker(dir);
    if (marker !== null && marker !== own) return dir;
    if (level >= 3) return null;
    let entries = [];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (_) {
      return null;
    }
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.isSymbolicLink()) continue;
      const found = walk(path.join(dir, entry.name), level + 1);
      if (found) return found;
    }
    return null;
  };
  return walk(path.resolve(root), 0);
}

// Что очищает полное удаление: библиотеку и папку вокруг каталога программы
// с маркером библиотеки (её раскладку создаёт установщик, даже если игры
// потом переехали). Не трогается (blocked, с причиной why): каталог с другой
// установкой HLSR и любая библиотека у неподтверждённой (problem — маркер
// владения не совпал) или не-боевой сборки — у QA библиотека по умолчанию
// общая с боевым HLSR (C:\HLSR), а чья она, наверняка не узнать.
function libraryRoots(installDir, libraryRoot, identity, problem, cleanup = libraryCleanup) {
  const parent = path.dirname(path.resolve(installDir));
  const roots = [], blocked = [];
  for (const root of [libraryRoot, cleanup.readMarker(parent) ? parent : null]) {
    if (!root || roots.concat(blocked.map((item) => item.root)).some((other) => layout.isSameDir(other, root))) continue;
    if (problem) blocked.push({ root, why: "unverified", reason: `Игры не удалены: установку не удалось проверить (${problem}).` });
    else if (!isProduction(identity, problem))
      blocked.push({ root, why: "test", reason: `Игры не удалены: тестовая сборка ${identity.productName} не удаляет библиотеку ${root}, ею может пользоваться основной HLSR.` });
    else if (otherInstall(root, installDir)) blocked.push({ root, why: "other", reason: `Игры не удалены: в папке ${root} есть другая установка HLSR.` });
    else roots.push(root);
  }
  return { roots, blocked };
}

// GUID сборки из маркера владения каталога программы или null.
function markerGuid(installDir) {
  try {
    const guid = JSON.parse(readMarker(installDir)).appId;
    return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(guid) ? guid : null;
  } catch (_) {
    return null;
  }
}

// Сборка каталога программы: по списку файлов, если его GUID совпал с
// маркером владения. Иначе — по GUID маркера, и тогда она не боевая (appId
// неизвестен): ключи реестра другой сборки так не подставятся. Ни списка,
// ни маркера — боевая по ключам, но маркера нет, и ownershipProblem не даст
// ни полного удаления, ни чужих ключей.
function identityOf(installDir) {
  const guid = markerGuid(installDir);
  let id = null;
  try {
    id = layout.identity(fileList.read(installDir).app);
  } catch (_) {
    /* списка нет или он повреждён */
  }
  if (id && (!guid || id.guid === guid)) return id;
  return guid ? layout.identityOfGuid(guid) : layout.identity();
}

// Боевая сборка подтверждена: appId боевой и маркер владения совпал с реестром.
function isProduction(id, problem) {
  return id.appId === layout.PRODUCTION.appId && !problem;
}

// registry и env передаёт setup.js: без них совпадение маркера с реестром не
// проверяется (тесты, пробы), остальные проверки те же.
function plan({ installDir, libraryRoot, identity = identityOf(installDir), registry = null, env = process.env }) {
  // Список файлов и маркер владения должны указывать на одну сборку.
  const problem = markerGuid(installDir) !== identity.guid ? "маркер владения не от этой сборки"
    : registry ? ownershipProblem(installDir, registry.read(identity.installKey) || {}, env, identity) : null;
  const here = realDir(installDir); // сравнения с библиотекой — по полному пути
  const { roots, blocked } = libraryRoots(here, libraryRoot, identity, problem);
  const existing = roots.filter(exists);
  // Размер того, что удалится: свои папки библиотеки (без каталога программы
  // и чужих файлов); вложенная библиотека уходит с программой целиком.
  const ownedBytes = (root) => {
    if (layout.isInside(root, here)) return treeSize(root);
    try {
      return libraryCleanup.ownedEntries(root, here).targets.reduce((size, target) => size + treeSize(target), 0);
    } catch (_) {
      return 0;
    }
  };
  const bytes = existing.reduce((sum, root) => sum + ownedBytes(root), 0);
  return {
    managed: true,
    installRoot: installDir,
    installBytes: treeSize(installDir),
    libraryRoot,
    libraryRoots: existing,
    libraryExists: existing.length > 0,
    libraryBytes: bytes,
    // Переключателя нет из-за защиты: окно говорит почему (other, test, unverified).
    libraryBlocked: existing.length ? null : (blocked.find((item) => exists(item.root)) || {}).why || null,
  };
}

// Причина отказа удалять каталог программы или null.
function ownershipProblem(installDir, values, env, id) {
  const refused = layout.refusedDir(installDir, env);
  if (refused) return refused;
  let current = path.resolve(installDir);
  for (;;) {
    try {
      if (fs.lstatSync(current).isSymbolicLink()) return `ссылка на пути: ${current}`;
    } catch (_) {
      return `нет каталога ${current}`;
    }
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }
  if (!exists(path.join(installDir, layout.EXE))) return `нет ${layout.EXE}`;
  // Копия каталога (резервная, перенесённая руками) несёт тот же маркер:
  // установка — только та, на которую указывает реестр.
  const location = values && values.InstallLocation;
  if (!location || !sameDir(location, installDir)) return `это копия, а не установка: в реестре каталог ${location || "не указан"}`;
  const installId = values && values.HLSRInstallId;
  if (!layout.INSTALL_ID_RE.test(installId || "")) return "в реестре нет installId установки";
  let marker;
  try {
    marker = fs.readFileSync(path.join(installDir, layout.OWNER_MARKER), "utf8").replace(/[\r\n]+$/, "");
  } catch (_) {
    return "нет маркера владения";
  }
  if (marker !== id.ownerMarker(installId)) return "маркер владения не совпадает с реестром";
  return null;
}

// Каталог данных HLSR (hlsr, HLSR Updater…) прямо в %APPDATA% или
// %LOCALAPPDATA%, существующий и без ссылок на пути.
function ownDataDir(dir, env) {
  const full = path.resolve(String(dir || ""));
  if (!/^hlsr/i.test(path.basename(full)) || layout.refusedDir(full, env)) return false;
  if (![env.APPDATA, env.LOCALAPPDATA].some((root) => root && layout.isSameDir(root, path.dirname(full)))) return false;
  for (let current = full; ; current = path.dirname(current)) {
    try {
      if (fs.lstatSync(current).isSymbolicLink()) return false;
    } catch (_) {
      return false;
    }
    if (path.dirname(current) === current) return true;
  }
}

// Ответы очистки библиотеки (src/main/uninstall/library-cleanup.js).
function libraryWarning(result, codes) {
  if (result.ok) return null;
  if (result.code === codes.REFUSED_RUNNING) return `Игры не удалены: игра ещё запущена. Закройте её и удалите папку вручную: ${result.root}`;
  if (result.code === codes.PARTIAL) return `Часть файлов удалить не удалось. Оставшиеся файлы здесь: ${result.root}`;
  return `Игры не удалены: путь библиотеки не прошёл проверки безопасности (${result.reason || result.code}).`;
}

function uninstall(options) {
  const { installDir, libraryRoot, removeGames, registry, shortcuts, cleanup, identity: id, env = process.env } = options;
  const log = options.log || { info() {}, warn() {} };
  const warnings = [];
  const values = registry.read(id.installKey) || {};
  const problem = ownershipProblem(installDir, values, env, id);
  // Полное удаление (игры, данные) и уборка за боевой сборкой — только если
  // маркер владения совпал с реестром: иначе неизвестно, чья это установка.
  const full = removeGames === true && !problem;
  if (removeGames === true && problem) warnings.push(`Игры и данные не удалены: установку не удалось проверить (${problem}).`);
  const production = isProduction(id, problem);
  // Сравнения с библиотекой и ярлыками — по полному пути (короткие имена 8.3 раскрыты).
  const here = realDir(installDir);
  const parent = path.dirname(here);
  // Старые версии разрешали выбрать библиотеку внутри папки программы.
  const nested = !!libraryRoot && layout.isInside(libraryRoot, here);
  // Данные пользователя — только при полном удалении.
  const dataDirs = full ? (options.dataDirs || []).filter((dir) => ownDataDir(dir, env)) : [];

  let gamesRemoved = false, parentPurged = false;
  if (full) {
    const { roots, blocked } = libraryRoots(here, libraryRoot, id, problem, cleanup);
    for (const item of blocked) warnings.push(item.reason);
    for (const root of roots) {
      // Вложенная библиотека прежних версий уходит с папкой программы, а
      // старая библиотека в %APPDATA%\hlsr\library — с данными, если в ней
      // не запущена игра.
      const inProgram = nested && layout.isSameDir(root, libraryRoot);
      const inData = dataDirs.find((dir) => layout.isInside(root, dir));
      if (inProgram || inData) {
        if (inProgram && problem) continue;
        if (cleanup.runningGames(root).length) {
          warnings.push(libraryWarning({ ok: false, code: cleanup.CODES.REFUSED_RUNNING, root }, cleanup.CODES));
          if (inData) dataDirs.splice(dataDirs.indexOf(inData), 1);
        } else gamesRemoved = true;
        continue;
      }
      const result = cleanup.cleanupLibrary(root, { purge: true, installDir: here, env, steamLibraries: options.steamLibraries, ownMarker: ownMarkerOf(installDir) });
      const warning = libraryWarning(result, cleanup.CODES);
      if (warning) warnings.push(warning);
      const left = result.left || [];
      if (left.length)
        warnings.push(`Оставлено в ${result.root} (${left.length}), удаление это не трогает: ${left.slice(0, 10).join(", ")}${left.length > 10 ? ", …" : ""}.`);
      if (libraryRoot && layout.isSameDir(root, libraryRoot)) gamesRemoved = result.ok;
      if (layout.isSameDir(root, parent)) parentPurged = result.ok;
    }
  }
  // Даже при отказе очистки игр нельзя затем удалить их вместе с папкой программы.
  const keepNestedLibrary = nested && !gamesRemoved;

  const exe = path.join(here, layout.EXE);
  for (const file of [shortcuts.startMenu, shortcuts.desktop]) {
    if (!shortcuts.pointsTo(file, exe)) continue;
    try {
      fs.unlinkSync(file);
    } catch (error) {
      warnings.push(`Ярлык не удалён: ${file}`);
    }
  }

  try {
    // Ключи этой сборки — если маркер совпал с реестром или ключ указывает
    // на этот каталог (InstallLocation); ключи другой установки не трогаем.
    const changes = [];
    for (const [key, keyValues] of [[id.uninstallKey, registry.read(id.uninstallKey)], [id.installKey, registry.read(id.installKey)]]) {
      if (!keyValues) continue;
      if (!problem || (keyValues.InstallLocation && sameDir(keyValues.InstallLocation, installDir))) changes.push({ key, remove: true });
      else warnings.push(`Запись реестра другой установки (${keyValues.InstallLocation || "каталог не указан"}) не тронута: ${key}`);
    }
    // InstallerOrigin (язык страницы загрузки, initial-language.js) один на
    // все сборки: его убирает только боевая, ключ — если в нём больше ничего нет.
    const origin = production ? registry.read(layout.ORIGIN_KEY) : null;
    if (origin)
      changes.push(Object.keys(origin).every((name) => name === "InstallerOrigin")
        ? { key: layout.ORIGIN_KEY, remove: true } : { key: layout.ORIGIN_KEY, values: { InstallerOrigin: null } });
    if (changes.length) registry.apply(changes);
  } catch (error) {
    warnings.push(`Запись в «Программах и компонентах» не удалена: ${error.message}`);
  }
  // Electron удаляет ключ протокола, только если он ведёт к этому HLSR.exe.
  if (DISCORD_PROTOCOL && options.removeProtocolClient && !options.removeProtocolClient(DISCORD_PROTOCOL))
    log.warn(`[uninstall] протокол ${DISCORD_PROTOCOL} не удалён`);

  // Кеш обновлений лаунчера — служебный, не данные пользователя. Свой у
  // каждой сборки (<имя приложения>-updater): удаление QA-сборки не должно
  // стирать кеш боевого HLSR (hlsr-updater).
  const updaterCache = /^[A-Za-z0-9._-]+-updater$/.test(String(options.updaterCacheName || ""))
    ? options.updaterCacheName : "hlsr-updater";
  if (env.LOCALAPPDATA) {
    try {
      fs.rmSync(path.join(env.LOCALAPPDATA, updaterCache), { recursive: true, force: true });
    } catch (error) {
      log.warn(`[uninstall] кеш обновлений: ${error.message}`);
    }
  }
  // Копии HLSRUpdater.exe в TEMP (updater-launch.js): лаунчер убирает их
  // только через сутки. tempRoot задаёт setup.js, тесты — свою папку.
  if (options.tempRoot && !problem) removeUpdaterStages(options.tempRoot, id, log);

  // Профиль Chromium апдейтера (%APPDATA%\HLSR Updater) служебный; имя у него
  // от боевого HLSR, QA-сборка его только делит — убирает боевая, всегда.
  const data = dataDirs.concat(production && env.APPDATA ? [path.join(env.APPDATA, updaterMode.APP_NAME)] : []);
  const removeData = data.filter((dir, index) => ownDataDir(dir, env) && data.findIndex((other) => layout.isSameDir(other, dir)) === index);

  if (keepNestedLibrary) warnings.push(`Папка программы оставлена, чтобы сохранить вложенную библиотеку игр: ${libraryRoot}.`);
  else if (problem) warnings.push(`Каталог программы не удалён (${problem}): ${installDir}. Удалите его вручную, если он больше не нужен.`);
  log.info(`[uninstall] HLSR удалён${problem ? `, каталог программы оставлен: ${problem}` : ""}`);
  const removeProgram = problem || keepNestedLibrary ? null : installDir;
  // Папка вокруг каталога программы уходит, только если после него
  // опустела: очищенная библиотека или папка с именем продукта (C:\HLSR).
  const ownParent = parentPurged || path.basename(parent).toLowerCase() === id.productName.toLowerCase();
  return {
    status: warnings.length ? "partial" : "ok", warnings, gamesRemoved, removeProgram, removeData,
    removeParent: removeProgram && ownParent ? parent : null, ownMarker: ownMarkerOf(installDir),
  };
}

// Свои копии апдейтера в TEMP: в копии лежит маркер владения установки
// (updater-launch.stage копирует каталог программы). Чужие и копии без
// маркера остаются; работающую копию держит её HLSRUpdater.exe — её тоже не трогаем.
function removeUpdaterStages(tempRoot, id, log) {
  let names = [];
  try {
    names = fs.readdirSync(tempRoot);
  } catch (_) {
    return;
  }
  for (const name of names) {
    const dir = path.join(tempRoot, name);
    if (!UPDATER_STAGE_RE.test(name)) continue;
    let marker = null;
    try {
      marker = JSON.parse(readMarker(dir));
    } catch (_) {
      /* маркера нет — чья копия, неизвестно */
    }
    if (!marker || marker.appId !== id.guid) continue;
    // Запущенный exe Windows не даёт открыть на запись.
    try {
      fs.closeSync(fs.openSync(path.join(dir, UPDATER_EXE), "r+"));
    } catch (error) {
      if (error.code !== "ENOENT") continue;
    }
    try {
      fs.rmSync(dir, { recursive: true, force: true });
    } catch (error) {
      log.warn(`[uninstall] копия апдейтера ${name}: ${error.message}`);
    }
  }
}

// PowerShell ждёт, пока этот процесс выйдет и отпустит файлы, и удаляет
// каталог программы и данные (HLSR_REMOVE_DATA, через «|»: в путях Windows
// его не бывает), а затем папку HLSR_REMOVE_PARENT, если та опустела.
// Раньше это делал cmd.exe с ping для паузы: процесс запускается
// отвязанным (detached), консоли у него нет, и каждый ping Windows
// открывала в новом видимом окне. PowerShell ждёт сам (Start-Sleep) и
// дочерних программ не запускает. Пути проверены ownershipProblem и
// ownDataDir и передаются переменными окружения, а не текстом команды.
// Remove-NoFollow не заходит в junction и symlink: удаляется сама ссылка,
// её цель остаётся. Каталог с маркером владения другой установки
// (HLSR_OWN_MARKER — свой) или с нечитаемым маркером не трогается. Read-only снимается только у
// самого объекта и не снимается у файла с жёсткими ссылками: атрибуты у
// них общие с файлом снаружи, такой файл остаётся. Путь, который уже
// удалён, из списка уходит: если его тут же создаст новая установка, цикл
// её не тронет. С relaunch после удаления снова открывается установщик
// («Назад» в окне удаления): только когда каталога уже нет.
const REMOVAL_SCRIPT =
  "$ErrorActionPreference = 'SilentlyContinue'; " +
  "function Remove-NoFollow($p) { $i = Get-Item -LiteralPath $p -Force; if (-not $i) { return }; " +
  "$link = $i.Attributes -band 1024; " +
  "if ($i.PSIsContainer -and -not $link) { " +
  "$m = Join-Path $p '.hlsr-install-owner.json'; " +
  "if (Test-Path -LiteralPath $m) { try { if ([IO.File]::ReadAllText($m).TrimEnd() -ne $env:HLSR_OWN_MARKER) { return } } catch { return } }; " +
  "Get-ChildItem -LiteralPath $p -Force | ForEach-Object { Remove-NoFollow $_.FullName } }; " +
  "if (($i.Attributes -band 1) -and $i.LinkType -ne 'HardLink') { $i.Attributes = $i.Attributes -band -bnot 1 }; " +
  "try { $i.Delete() } catch {} }; " +
  "for ($i = 0; $i -lt 60 -and $env:HLSR_WAIT_PID -and (Get-Process -Id $env:HLSR_WAIT_PID); $i++) { Start-Sleep -Seconds 1 }; " +
  "$d = $env:HLSR_REMOVE_DIR; " +
  "$all = @(@($d) + @($env:HLSR_REMOVE_DATA -split '\\|') | Where-Object { $_ -and (Test-Path -LiteralPath $_) }); " +
  "for ($i = 0; $i -lt 60 -and $all.Count; $i++) { $all | ForEach-Object { Remove-NoFollow $_ }; " +
  "$all = @($all | Where-Object { Test-Path -LiteralPath $_ }); if ($all.Count) { Start-Sleep -Seconds 1 } }; " +
  "$q = $env:HLSR_REMOVE_PARENT; " +
  "if ($q -and (Test-Path -LiteralPath $q) -and -not (Get-ChildItem -LiteralPath $q -Force)) { Remove-NoFollow $q }; " +
  "if ($env:HLSR_RELAUNCH -and -not ($d -and (Test-Path -LiteralPath $d))) { Start-Process -FilePath $env:HLSR_RELAUNCH }";

// installDir может быть null, если удалять нужно только данные.
function removalCommand(installDir, env = process.env, relaunch = null, { data = [], parent = null, pid = null, ownMarker = null } = {}) {
  const paths = [installDir, ...data, parent].filter((value) => value !== null && value !== undefined);
  if (!paths.length) throw new Error("Недопустимый путь: удалять нечего");
  for (const value of paths)
    if (!path.win32.isAbsolute(String(value)) || /[\r\n\0|]/.test(value)) throw new Error(`Недопустимый путь: ${value}`);
  const systemRoot = env.SystemRoot || env.WINDIR || "C:\\Windows";
  return {
    file: path.join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"),
    args: ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-Command", REMOVAL_SCRIPT],
    env: {
      ...env, HLSR_REMOVE_DIR: installDir || "", HLSR_REMOVE_DATA: data.join("|"), HLSR_REMOVE_PARENT: parent || "",
      HLSR_WAIT_PID: pid ? String(pid) : "", HLSR_OWN_MARKER: ownMarker || "", ...(relaunch ? { HLSR_RELAUNCH: relaunch } : {}),
    },
  };
}

// Как запустить скрипт, чтобы он пережил выход HLSR.exe. PowerShell с
// DETACHED_PROCESS (spawn с detached: true) выходит с кодом 0, не выполнив
// -Command, и каталог программы не удалялся вовсе. Обычный дочерний процесс
// Node (libuv) завершается вместе с родителем. Поэтому короткий PowerShell с
// (скрытой) консолью запускает скрипт через Start-Process и сразу выходит:
// внук в задание libuv не входит и доживает своё. Скрипт идёт через
// -EncodedCommand, путь — через окружение, как и прежде.
function launcherCommand(command) {
  const script = command.args[command.args.length - 1];
  const encoded = Buffer.from(script, "utf16le").toString("base64");
  const launch = "Start-Process -FilePath $env:HLSR_POWERSHELL -WindowStyle Hidden -ArgumentList " +
    `'-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','${encoded}'`;
  return {
    file: command.file,
    args: ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", launch],
    env: { ...command.env, HLSR_POWERSHELL: command.file },
  };
}

function scheduleRemoval(installDir, options = {}) {
  const run = options.spawnSync || spawnSync;
  const command = launcherCommand(removalCommand(installDir, options.env || process.env, options.relaunch || null,
    { data: options.data || [], parent: options.parent || null, pid: options.pid === undefined ? process.pid : options.pid,
      ownMarker: options.ownMarker || null }));
  const result = run(command.file, command.args, {
    cwd: os.tmpdir(), stdio: "ignore", windowsHide: true, timeout: 30000, env: command.env,
  });
  if (result && result.error) throw result.error;
  if (result && result.status !== 0) throw new Error(`скрипт удаления не запущен (код ${result.status})`);
  return result;
}

module.exports = { plan, uninstall, identityOf, ownershipProblem, ownDataDir, removalCommand, launcherCommand, scheduleRemoval, DISCORD_PROTOCOL };
