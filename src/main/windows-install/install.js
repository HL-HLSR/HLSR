"use strict";

// Установка HLSR в Windows.
//
// HLSR-Setup-<версия>.exe — сам лаунчер в самораспаковывающемся exe
// (portable-цель electron-builder): exe распаковывает его во временный
// каталог и запускает, лаунчер видит PORTABLE_EXECUTABLE_FILE и вместо
// себя открывает окно установщика (src/installer) — то же, что в Linux.
// Установка — копирование распакованного каталога в C:\HLSR\Launcher по
// списку файлов из сборки (file-list.js) со сверкой размера и SHA-256.
//
// Замена атомарная. Новая версия собирается рядом, в .Launcher.hlsr-new-*,
// затем установленный каталог переименовывается в .Launcher.hlsr-old-*, а
// новый встаёт на его место. Пока открыт хоть один файл старой версии,
// Windows не даёт переименовать каталог, и рабочая установка остаётся целой.
const crypto = require("crypto");
const path = require("path");
const { Transform } = require("stream");
const { pipeline } = require("stream/promises");
const fs = require("../launcher-update/fs-compat");
const layout = require("./layout");
const fileList = require("./file-list");

// Без этих файлов лаунчер работает, не будет только оверлея LiveSplit: файлы,
// которые не прочитать (их мог унести антивирус), установку не останавливают.
const OPTIONAL_PREFIX = "resources/livesplit-overlay/";
const IN_USE_CODES = new Set(["EBUSY", "EPERM", "EACCES", "ENOTEMPTY"]);
// Файл забрал антивирус: Windows не даёт его открыть (EPERM, EACCES, EBUSY),
// libuv не знает кодов ERROR_VIRUS_INFECTED и ERROR_VIRUS_DELETED (UNKNOWN),
// или распакованный файл пропал. Источник и стадия — свои свежие каталоги,
// поэтому отказ доступа здесь — не права папки.
const ANTIVIRUS_CODES = new Set(["UNKNOWN", "EPERM", "EACCES", "EBUSY"]);
const LEFTOVER_RE = /^\.(.+)\.hlsr-(?:new|old)-[0-9a-f]{8}$/;
// createdBy: "installer" — папку библиотеки создал сам установщик: полное
// удаление уносит её свои папки целиком (library-cleanup). В уже
// существующую папку (её мог завести человек) — маркер без отметки.
const LIBRARY_MARKER = '{"schema":1,"id":"hlsr-library","createdBy":"installer"}';
const LIBRARY_MARKER_PLAIN = '{"schema":1,"id":"hlsr-library"}';
// BUILTIN\Users в виде SID: имя группы зависит от языка Windows, SID — нет.
const USERS_SID = "*S-1-5-32-545";

class InstallError extends Error {
  // cause — исходная ошибка ОС: по её коду окно объясняет, что случилось.
  constructor(code, message, cause) {
    super(message);
    this.code = code;
    if (cause) this.cause = cause;
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function copyVerified(sourceRoot, targetRoot, item, signal) {
  const parts = item.path.split("/");
  const from = path.join(sourceRoot, ...parts);
  const to = path.join(targetRoot, ...parts);
  await fs.promises.mkdir(path.dirname(to), { recursive: true });
  const hash = crypto.createHash("sha256");
  let size = 0;
  const meter = new Transform({
    transform(chunk, _encoding, callback) {
      hash.update(chunk);
      size += chunk.length;
      callback(null, chunk);
    },
  });
  try {
    await pipeline(fs.createReadStream(from), meter, fs.createWriteStream(to, { flags: "wx" }), { signal });
  } catch (error) {
    if (error.name === "AbortError") throw new InstallError("ABORTED", "Установка отменена");
    const blocked = ANTIVIRUS_CODES.has(error.code) || /virus/i.test(error.message) || !fs.existsSync(from);
    // Сырой текст ошибки — только в журнал: окно объясняет блокировку своими словами.
    const failure = blocked
      ? new InstallError("ANTIVIRUS", `Антивирус заблокировал или удалил файл ${item.path} (${error.code || "нет файла"})`)
      : new InstallError("COPY_FAILED", `Не удалось скопировать ${item.path}: ${error.message}`);
    failure.file = item.path;
    failure.cause = error;
    throw failure;
  }
  if (size !== item.size || hash.digest("hex") !== item.sha256)
    throw new InstallError("CORRUPT", `Файл установщика повреждён: ${item.path}. Скачайте установщик заново.`);
  return size;
}

function existingKind(dir) {
  let stat;
  try {
    stat = fs.lstatSync(dir);
  } catch (error) {
    if (error.code === "ENOENT") return "none";
    throw error;
  }
  if (stat.isSymbolicLink() || !stat.isDirectory()) return "foreign";
  const names = fs.readdirSync(dir).map((name) => name.toLowerCase());
  if (!names.length) return "empty";
  if (names.includes(layout.EXE.toLowerCase()) || names.includes(layout.OWNER_MARKER)) return "hlsr";
  return "foreign";
}

// Остатки прерванных установок рядом с каталогом программы.
function sweepLeftovers(parent, name, log) {
  let entries = [];
  try {
    entries = fs.readdirSync(parent);
  } catch (_) {
    return;
  }
  for (const entry of entries) {
    const match = LEFTOVER_RE.exec(entry);
    if (!match || match[1].toLowerCase() !== name.toLowerCase()) continue;
    // В старой папке могут оставаться единственные копии пользовательских
    // файлов после прерванной замены. Автоматически её не удаляем.
    if (/\.hlsr-old-/.test(entry)) {
      if (!fs.existsSync(path.join(parent, name)))
        throw new InstallError("RECOVERY_REQUIRED", `Сохранилась прежняя установка: ${path.join(parent, entry)}. Восстановите её файлы перед обновлением.`);
      log.warn(`[setup] резервная папка сохранена: ${path.join(parent, entry)}`);
    }
  }
  for (const entry of entries) {
    const match = LEFTOVER_RE.exec(entry);
    if (!match || match[1].toLowerCase() !== name.toLowerCase()) continue;
    if (/\.hlsr-old-/.test(entry)) continue;
    try {
      fs.rmSync(path.join(parent, entry), { recursive: true, force: true });
    } catch (error) {
      log.warn(`[setup] не удалось убрать ${entry}: ${error.message}`);
    }
  }
}

function installedFiles(root, id) {
  try {
    const list = fileList.read(root);
    if (layout.identity(list.app).guid === id.guid)
      return new Set(list.files.map((item) => item.path.toLowerCase()));
  } catch (_) {
    // У NSIS списка нет. Неизвестные файлы сохраняются: это могут быть
    // игры, конфиги и демо, а не файлы прежней версии лаунчера.
  }
  return new Set();
}

async function preserveUserFiles(root, stage, list, id, libraryRoot, signal) {
  const oldFiles = installedFiles(root, id);
  const newFiles = new Set(list.files.map((item) => item.path.toLowerCase()));
  const metadata = new Set([layout.OWNER_MARKER.toLowerCase(), fileList.RELATIVE.toLowerCase()]);
  const obsolete = new Set([`uninstall ${id.productName}.exe`.toLowerCase(), "uninstallericon.ico"]);
  const nestedLibrary = layout.isInside(libraryRoot, root) && !layout.isSameDir(root, libraryRoot) ? path.resolve(libraryRoot) : null;

  async function walk(relative) {
    for (const entry of fs.readdirSync(path.join(root, relative), { withFileTypes: true })) {
      if (signal && signal.aborted) throw new InstallError("ABORTED", "Установка отменена");
      const child = relative ? `${relative}/${entry.name}` : entry.name;
      const from = path.join(root, ...child.split("/"));
      const to = path.join(stage, ...child.split("/"));
      const key = child.toLowerCase();
      const libraryFile = nestedLibrary && layout.isInside(from, nestedLibrary);
      if (newFiles.has(key)) {
        if (libraryFile || entry.isDirectory() || entry.isSymbolicLink())
          throw new InstallError("PRESERVE_FAILED", `Файлы пользователя пересекаются с новой версией: ${from}. Переместите их за пределы папки лаунчера.`);
        continue;
      }
      if (metadata.has(key)) {
        if (entry.isDirectory() || libraryFile)
          throw new InstallError("PRESERVE_FAILED", `Служебный файл новой версии занимает папку пользователя: ${from}`);
        continue;
      }
      // Если прежний файл заменили личной папкой или ссылкой, список
      // прошлой версии уже не даёт права удалить её содержимое.
      if (!libraryFile && entry.isFile() && (obsolete.has(key) || oldFiles.has(key))) continue;
      let destination;
      try { destination = fs.lstatSync(to); } catch (error) { if (error.code !== "ENOENT") throw error; }
      if (entry.isDirectory()) {
        if (destination && (!destination.isDirectory() || destination.isSymbolicLink()))
          throw new InstallError("PRESERVE_FAILED", `Новая версия занимает папку пользователя: ${from}`);
        fs.mkdirSync(to, { recursive: true });
        await walk(child);
      } else {
        if (destination) throw new InstallError("PRESERVE_FAILED", `Новая версия занимает файл пользователя: ${from}`);
        fs.mkdirSync(path.dirname(to), { recursive: true });
        if (entry.isSymbolicLink()) {
          const link = fs.readlinkSync(from);
          let directory = false;
          try { directory = fs.statSync(from).isDirectory(); } catch (_) { /* сохраняем и оборванную ссылку */ }
          fs.symlinkSync(link, to, directory && process.platform === "win32" ? "junction" : undefined);
        } else if (entry.isFile()) {
          try { await fs.promises.link(from, to); }
          catch (_) {
            // На FAT/exFAT и некоторых сетевых дисках жёстких ссылок нет.
            // Копирование с EXCL тоже не перезапишет чужой файл стадии.
            await fs.promises.copyFile(from, to, fs.constants.COPYFILE_EXCL);
          }
        } else {
          throw new InstallError("PRESERVE_FAILED", `Не удалось сохранить объект пользователя: ${from}`);
        }
      }
    }
  }
  try { await walk(""); }
  catch (error) {
    if (error instanceof InstallError) throw error;
    throw new InstallError("PRESERVE_FAILED", `Пользовательские файлы не перенесены; прежняя установка сохранена: ${error.message}`, error);
  }
}

async function renameWithRetry(from, to, { attempts, delayMs, wait }) {
  for (let attempt = 1; ; attempt++) {
    try {
      fs.renameSync(from, to);
      return;
    } catch (error) {
      if (attempt >= attempts || !IN_USE_CODES.has(error.code)) throw error;
      await wait(delayMs);
    }
  }
}

function newInstallId() {
  return `{${crypto.randomUUID().toUpperCase()}}`;
}

// Раскладка библиотеки, как её создавал прежний установщик. Каталоги
// только создаются, маркер пишется, если его нет: игры и чужие поля не
// трогаем. Ошибки не роняют установку — лаунчер создаст всё сам.
function ensureLibraryLayout({ root, created, run, env, log }) {
  try {
    for (const dir of ["", "Games", "Downloads", ".hlsr"]) fs.mkdirSync(path.join(root, dir), { recursive: true });
    const marker = path.join(root, ".hlsr", "library.json");
    // Через ссылку на месте .hlsr маркер не пишется: он попал бы в чужую папку.
    if (!fs.lstatSync(path.join(root, ".hlsr")).isSymbolicLink() && !fs.existsSync(marker))
      fs.writeFileSync(marker, created ? LIBRARY_MARKER : LIBRARY_MARKER_PLAIN, { flag: "wx" });
  } catch (error) {
    log.warn(`[setup] раскладка библиотеки: ${error.message}`);
    return;
  }
  // Обычный пользователь ставит игры без прав администратора, даже если
  // каталог когда-то создал администратор.
  if (!run) return;
  const icacls = path.join(env.SystemRoot || env.WINDIR || "C:\\Windows", "System32", "icacls.exe");
  const result = run(icacls, [root, "/grant", `${USERS_SID}:(OI)(CI)M`]);
  if (result.status !== 0) log.warn(`[setup] icacls ${root}: код ${result.status}`);
}

function uninstallEntry({ id, installDir, version, sizeBytes }) {
  const exe = path.join(installDir, layout.EXE);
  return {
    DisplayName: `${id.productName} ${version}`,
    DisplayVersion: version,
    DisplayIcon: `${exe},0`,
    Publisher: layout.PUBLISHER,
    InstallLocation: installDir,
    UninstallString: `"${exe}" ${layout.UNINSTALL_FLAG}`,
    QuietUninstallString: `"${exe}" ${layout.UNINSTALL_FLAG} /S`,
    NoModify: { dword: 1 },
    NoRepair: { dword: 1 },
    EstimatedSize: { dword: Math.ceil(sizeBytes / 1024) },
  };
}

// Метка страницы загрузки: HLSR-Setup-<версия>-ru.exe / -en.exe. Файл без
// суффикса метки не оставляет — язык тогда берётся из системы.
function originOf(setupFile) {
  const match = /-(ru|en)\.exe$/i.exec(path.basename(String(setupFile || "")));
  return match ? match[1].toLowerCase() : null;
}

function wantsDesktopShortcut(values, update, choice) {
  if (!update && typeof choice === "boolean") return choice;
  return !values || values[layout.DESKTOP_VALUE] !== "0";
}

async function install(options) {
  const {
    sourceDir, installDir, registry, shortcuts, signal,
    env = process.env, update = false, desktopShortcut = null, setupFile = null,
    libraryRoot = layout.LIBRARY_ROOT,
  } = options;
  const log = options.log || { info() {}, warn() {} };
  const wait = options.sleep || sleep;
  const onProgress = options.onProgress || (() => {});

  const refused = layout.refusedDir(installDir, env);
  if (refused) throw new InstallError("BAD_DIR", `Нельзя установить HLSR сюда: ${refused}`);
  const source = path.resolve(sourceDir);
  const target = path.resolve(installDir);
  if (layout.isInside(source, target) || layout.isInside(target, source))
    throw new InstallError("BAD_DIR", "Установщик запущен из каталога установки");

  const list = fileList.read(source);
  const id = layout.identity(list.app);
  const kind = existingKind(target);
  if (kind === "foreign")
    throw new InstallError("BAD_DIR", `Каталог ${target} занят другими файлами — HLSR туда не ставится`);

  const parent = path.dirname(target);
  const name = path.basename(target);
  // Была ли папка библиотеки до установщика (C:\HLSR — она же родитель каталога программы).
  const libraryCreated = !fs.existsSync(libraryRoot);
  // Родитель создаётся, только если его нет: для D:\hlsr это корень диска,
  // и mkdirSync("D:\\", { recursive: true }) в Windows падает с EPERM.
  if (!fs.existsSync(parent)) fs.mkdirSync(parent, { recursive: true });
  sweepLeftovers(parent, name, log);

  const values = registry.read(id.installKey) || {};
  const installId = layout.INSTALL_ID_RE.test(values.HLSRInstallId || "") ? values.HLSRInstallId : newInstallId();

  const suffix = () => crypto.randomBytes(4).toString("hex");
  const stage = path.join(parent, `.${name}.hlsr-new-${suffix()}`);
  const total = list.files.reduce((sum, item) => sum + item.size, 0);
  let done = 0;
  const skipped = [];
  try {
    fs.mkdirSync(stage);
    for (const item of list.files) {
      if (signal && signal.aborted) throw new InstallError("ABORTED", "Установка отменена");
      try {
        done += await copyVerified(source, stage, item, signal);
      } catch (error) {
        if (error.cause) log.warn(`[setup] ${error.message}: ${error.cause.message}`);
        if (!["COPY_FAILED", "ANTIVIRUS"].includes(error.code) || !item.path.startsWith(OPTIONAL_PREFIX)) throw error;
        fs.rmSync(path.join(stage, ...item.path.split("/")), { force: true });
        skipped.push(path.posix.basename(item.path));
        done += item.size;
      }
      onProgress(done, total);
    }
    if (kind !== "none" && kind !== "empty")
      await preserveUserFiles(target, stage, list, id, libraryRoot, signal);
    if (signal && signal.aborted) throw new InstallError("ABORTED", "Установка отменена");
    // Отмена разрешена, пока прежняя установка ещё не затронута.
    if (options.onCommit) options.onCommit();
    // Список едет в установку: по нему удаление узнаёт identity сборки.
    fs.writeFileSync(path.join(stage, fileList.RELATIVE), JSON.stringify(list) + "\n", { flag: "wx" });
    fs.writeFileSync(path.join(stage, layout.OWNER_MARKER), id.ownerMarker(installId), { flag: "wx" });

    let old = null;
    if (kind !== "none") {
      old = path.join(parent, `.${name}.hlsr-old-${suffix()}`);
      try {
        // Обновление приходит, пока прежняя версия ещё завершается: ждём.
        await renameWithRetry(target, old, update ? { attempts: 60, delayMs: 1000, wait } : { attempts: 5, delayMs: 400, wait });
      } catch (error) {
        throw new InstallError("IN_USE",
          `Файлы HLSR заняты (${error.code}). Закройте HLSR и всё, что открыто из ${target}, и повторите.`, error);
      }
    }
    try {
      fs.renameSync(stage, target);
    } catch (error) {
      if (old) fs.renameSync(old, target);
      throw new InstallError("SWAP_FAILED", `Не удалось поставить новую версию: ${error.message}`, error);
    }
    if (old) {
      try {
        fs.rmSync(old, { recursive: true, force: true });
      } catch (error) {
        log.warn(`[setup] прежняя версия осталась в ${old}: ${error.message}`);
      }
    }
  } finally {
    fs.rmSync(stage, { recursive: true, force: true });
  }

  const warnings = [];
  if (skipped.length)
    warnings.push(`Оверлей LiveSplit работать не будет: не удалось скопировать ${skipped.join(", ")}, файлы мог заблокировать антивирус`);
  const createDesktop = wantsDesktopShortcut(values, update, desktopShortcut);
  const exe = path.join(target, layout.EXE);
  const origin = originOf(setupFile);
  try {
    registry.apply([
      {
        key: id.installKey,
        values: {
          InstallLocation: target,
          KeepShortcuts: "true",
          ShortcutName: id.shortcutName,
          HLSRInstallId: installId,
          [layout.DESKTOP_VALUE]: createDesktop ? "1" : "0",
          [layout.SEED_FLAG]: null,
        },
      },
      { key: id.uninstallKey, values: uninstallEntry({ id, installDir: target, version: list.version, sizeBytes: total }) },
    ].concat(origin ? [{ key: layout.ORIGIN_KEY, values: { InstallerOrigin: origin } }] : []));
  } catch (error) {
    warnings.push(`Запись в «Программы и компоненты» не создана: ${error.message}`);
  }

  // Ярлыки: при установке человеком — по его выбору, при обновлении
  // недостающие восстанавливаются, существующие не трогаются.
  const wanted = [[shortcuts.startMenu, true], [shortcuts.desktop, createDesktop]];
  for (const [file, want] of wanted) {
    if (!want || (update && shortcuts.exists(file))) continue;
    try {
      shortcuts.write(file, exe);
    } catch (error) {
      warnings.push(error.message);
    }
  }

  ensureLibraryLayout({ root: libraryRoot, created: libraryCreated, run: options.run, env, log });
  log.info(`[setup] HLSR ${list.version} установлен в ${target}`);
  return { installDir: target, installId, version: list.version, identity: id, warnings };
}

module.exports = { install, InstallError, existingKind, originOf, LEFTOVER_RE };
