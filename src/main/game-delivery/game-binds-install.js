"use strict";

const nodeFs = require("fs");
const path = require("path");
const crypto = require("crypto");

const cfgSearch = require("./cfg-search");
const { assertInside } = require("./paths");
const { DeliveryError } = require("./errors");
const { readJsonSafe, writeJsonAtomic } = require("./atomic");
const gameBinds = require("../../shared/game-binds");
const bindPresets = require("../../shared/bind-presets");
const hl1Guide = require("../../shared/hl1-guide");
const cfgImport = require("../../shared/cfg-import");
const cfgDocument = require("../../shared/cfg-document");
const { checkConfig } = require("../../shared/hl1-config-check");
const playerConfig = require("./player-config");

// Бинды игры на диске: чтение итоговых биндов для клавиатуры в настройках и
// запись переназначений лаунчера в hlsr-binds.cfg (см. src/shared/game-binds.js).
// Файлы ищутся так же, как в игре: сначала языковая папка GoldSrc
// (valve_WON_russian), потом папка мода (см. cfg-search.js).

// Бинды игрока в той кодировке, в какой их пишет игра.
function readInstallBinds(installPath, gameDirectory, engine, options = {}) {
  const fs = options.fs || nodeFs;
  const profile = gameBinds.engineOf(engine);
  let files;
  try {
    files = cfgSearch.filesOf(installPath, gameDirectory, engine, { fs });
  } catch (error) {
    return gameBinds.readBinds(() => null, profile.roots);
  }
  return gameBinds.readBinds((name) => {
    const nested = String(name).replace(/\\/g, "/").replace(/^\/+/, "");
    if (!nested.includes("/") && !files.exists(name)) return null;
    try {
      // exec scripts/jump.cfg — файл в подпапке мода (импорт под своими путями).
      const file = nested.includes("/") ? importTarget(files, nested, fs) : files.pathOf(name);
      const stat = fs.statSync(file);
      if (!stat.isFile() || stat.size > 1024 * 1024) return null;
      return fs.readFileSync(file, profile.encoding);
    } catch (error) {
      return null;
    }
  }, profile.roots);
}

// Строка exec добавляется в конец файла один раз. Остальное в файле игрока
// лаунчер не трогает.
function ensureExecLine(file, fs, execLine, encoding) {
  let text = "";
  try {
    text = fs.readFileSync(file, encoding);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  if (text.split(/\r?\n/).some((line) => line.trim().toLowerCase() === execLine)) return false;
  const eol = text.includes("\r\n") || encoding === "latin1" ? "\r\n" : "\n";
  const suffix = text.length === 0 || text.endsWith("\n") ? "" : eol;
  fs.writeFileSync(file, `${text}${suffix}${execLine}${eol}`, encoding);
  return true;
}

// Alias назначенных скриптов. Скриптам гайда HL1 подставляются
// чувствительность и m_pitch игрока из config.cfg, а тем, что работают через game.cfg,
// — пустой auto_alias: до нажатия клавиши скрипта загрузка карты ничего не
// делает.
function scriptAliases(installPath, gameId, gameDirectory, engine, clean, fs) {
  const aliases = bindPresets.aliasesFor(gameId, clean);
  const autoAlias = bindPresets.needsAutoAlias(gameId, clean);
  if (!aliases.some(([, value]) => hl1Guide.hasPlayerToken(value)) && !autoAlias)
    return { aliases, autoAlias };
  const cvars = readInstallBinds(installPath, gameDirectory, engine, { fs }).cvars;
  const result = aliases.map(([name, value]) => [name, hl1Guide.withPlayerCvars(value, cvars)]);
  if (autoAlias) result.unshift([hl1Guide.AUTO_ALIAS, ""]);
  return { aliases: result, autoAlias };
}

const aliasLine = ([name, value]) => (value ? `alias ${name} "${value}"` : `alias ${name}`);
// Строка alias с токенами игрока как регулярное выражение: токен — любое число.
function lineMatcher(line) {
  const parts = line.split(/(\{sensitivity\}|\{m_pitch\})/);
  const source = parts.map((part, index) => (index % 2 ? "-?[\\d.]+" : part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))).join("");
  return new RegExp(`^${source}$`);
}

// Правит файл игрока на месте: по строке на бинд, по блоку на alias скрипта.
// previous — прежние переназначения. Пишется только разница с ними (ruling R15):
// клавиша, чьё переназначение не менялось, в файле не трогается (её могли поправить
// руками или задать импортом). Пропавшее переназначение уходит из файла, а пропавший
// возврат (command = base) — нет: игра уже держит base, строку игрока не удаляем.
// Блок, который больше не нужен, но правлен руками, остаётся (keptEdited),
// пока не передан removeEdited. settings нужны для снимка персонализации,
// который вызывают скрипты гайда (exec hlsr-personalization.cfg).
function applyBinds(installPath, gameId, gameDirectory, overrides, options = {}) {
  const engine = bindPresets.engineFor(gameId);
  if (!engine || !gameDirectory) return { status: "skipped", keptEdited: [] };
  const fs = options.fs || nodeFs;
  const profile = gameBinds.engineOf(engine);
  const via = { fs };
  if ("language" in options) via.language = options.language;
  const current = playerConfig.read(installPath, gameId, gameDirectory, via);
  const clean = gameBinds.sanitizeOverrides(overrides);
  const before = gameBinds.sanitizeOverrides(options.previous);
  const docKey = (key) => `bind:${key === ";" && engine !== "goldsrc" ? "semicolon" : key.toLowerCase()}`;

  let doc = cfgDocument.parse(current.text);
  for (const [key, entry] of Object.entries(before))
    if (!clean[key] && entry.command !== entry.base) doc = cfgDocument.removeKey(doc, docKey(key));
  for (const [key, { command }] of Object.entries(clean)) {
    if (before[key] && before[key].command === command) continue;
    if (command) {
      doc = cfgDocument.set(doc, docKey(key), playerConfig.toFileText(gameId, command), "binds");
    } else {
      doc = cfgDocument.insertLines(cfgDocument.removeKey(doc, docKey(key)), "binds", [
        `unbind "${key === ";" && engine !== "goldsrc" ? "semicolon" : key}"`,
      ]);
    }
  }

  const { aliases, autoAlias } = scriptAliases(installPath, gameId, gameDirectory, engine, clean, fs);
  const generated = (list) => new Map(list.map((item) => [item[0], aliasLine(item)]));
  const wanted = generated(aliases);
  // Свой alias run игрока (вне блока run) главнее нашего.
  if (cfgDocument.get(cfgDocument.removeBlock(doc, "run"), "alias:run")) wanted.delete("run");
  // Что лаунчер писал по прежним переназначениям: шаблон строки, где значения игрока
  // (чувствительность, m_pitch) могли быть другими, чем сейчас в config.cfg.
  const wrote = new Map();
  if (Object.keys(before).length) {
    const raw = bindPresets.aliasesFor(gameId, before);
    if (bindPresets.needsAutoAlias(gameId, before)) raw.unshift([hl1Guide.AUTO_ALIAS, ""]);
    for (const item of raw) wrote.set(item[0], lineMatcher(aliasLine(item)));
  }
  const isWritten = (block, id) => !!block && wrote.has(id) && block.body.length === 1 && wrote.get(id).test(block.body[0].trim());
  const keptEdited = [];
  for (const id of cfgDocument.blockIds(doc)) {
    // Блоки, о которых лаунчер не знает (переход, прошлые версии, руки), не трогаем и не спрашиваем.
    if (wanted.has(id) || !wrote.has(id)) continue;
    if (options.removeEdited || isWritten(cfgDocument.findBlock(doc, id), id)) doc = cfgDocument.removeBlock(doc, id);
    else keptEdited.push(id);
  }
  // Нужный блок правленый руками не откатываем: обновляем, только если его нет или он ещё как написал лаунчер.
  for (const [id, line] of wanted) {
    const block = cfgDocument.findBlock(doc, id);
    if (!block || isWritten(block, id)) doc = cfgDocument.setBlock(doc, id, [line], "scripts");
  }

  // Персонализация (если окно прислало прежние настройки) идёт в тот же документ:
  // одно чтение и одна запись файла игрока на сохранение.
  let changed = [];
  if (options.previousSettings && options.settings)
    ({ doc, changed } = playerConfig.applyPersonalization(doc, gameId, options.previousSettings, options.settings, current.text,
      { explicit: !!options.explicitPersonalization }));

  const out = cfgDocument.serialize(doc);
  let file = current.file;
  let sha256 = current.sha256;
  if (out !== current.text) {
    // expectedSha256 - отпечаток файла, как его прочитало окно настроек при открытии.
    const expectedSha256 = options.expectedSha256 !== undefined ? options.expectedSha256 : current.sha256;
    ({ file, sha256 } = playerConfig.write(installPath, gameId, gameDirectory, out, { ...via, expectedSha256 }));
  }

  const files = cfgSearch.filesOf(installPath, gameDirectory, engine, via);
  // Скрипт гайда зовёт снимок (alias deffov), а при запуске он не выполняется.
  const wantsSnapshot = [...wanted.values()].some((line) => playerConfig.SNAPSHOT_EXEC.test(line));
  if (options.settings && (wantsSnapshot || options.previousSettings))
    playerConfig.syncSnapshot(installPath, gameId, gameDirectory, out, options.settings, { ...via, force: wantsSnapshot });
  // game.cfg движок выполняет при каждой загрузке карты. Строку лаунчер
  // только добавляет: без назначенных скриптов пустой alias ничего не делает.
  if (autoAlias) {
    fs.mkdirSync(files.modDir, { recursive: true });
    ensureExecLine(files.pathOf(hl1Guide.GAME_CFG), fs, hl1Guide.AUTO_ALIAS, profile.encoding);
  }
  return { status: "applied", file, keptEdited, changed, sha256 };
}

// Импорт прошлых версий лаунчера (0.4.8): запись .hlsr/cfg-import.json. Новый импорт её
// не пишет, но переход с 0.4.8 (config-migration.js) читает её и раскрывает файлы.
const IMPORT_RECORD = path.join(".hlsr", "cfg-import.json");
// Хвост, который 0.4.8 дописывала в последний файл импорта (syncImportTrailer).
const IMPORT_TRAILER_MARK = "// HLSR: launcher files once more, after the imported config";
const IMPORT_MANIFEST = "import.json";
const BACKUP_STAMP = /^\d{8}-\d{6}$/;

// Что подключено импортом: { files: [{ path, created }], execs } — пути от
// папки мода. created — файла не было до импорта, иначе прежний лежит в
// .hlsr/cfg-import-backup. null — импорта нет.
function readImportRecord(installPath, options = {}) {
  const fs = options.fs || nodeFs;
  try {
    const record = JSON.parse(fs.readFileSync(path.join(installPath, IMPORT_RECORD), "utf8"));
    if (!record || !Array.isArray(record.files) || !Array.isArray(record.execs)) return null;
    const files = record.files.filter((item) => item && cfgImport.isSafeImportName(item.path) ||
      (item && cfgImport.ENGINE_RUN.test(String(item.path))));
    return { files, execs: record.execs.filter((name) => files.some((item) => item.path === name)) };
  } catch (error) {
    return null;
  }
}

// Путь файла импорта в папке мода. Игра под Windows и Wine не различает
// регистр: уже лежащий scripts/jump.cfg — тот же файл, что scripts/Jump.cfg.
function importTarget(files, relative, fs) {
  if (!relative.includes("/")) return files.pathOf(relative);
  let dir = files.modDir;
  for (const part of relative.split("/")) {
    let names = [];
    try { names = fs.readdirSync(dir); } catch (error) { /* папки ещё нет */ }
    dir = path.join(dir, names.find((name) => name.toLowerCase() === part.toLowerCase()) || part);
  }
  return assertInside(files.modDir, dir, "Файл импорта");
}

function readBytes(file, fs) {
  try {
    return fs.readFileSync(file);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

const sha256Of = (data) => crypto.createHash("sha256").update(data).digest("hex");

// Последняя копия импорта по играм ({ gameId: { stamp, sha256 } }, sha256 — её import.json)
// лежит в папке лаунчера (options.backupsFile), а не в папке игры: .hlsr/backup туда может
// подложить чужой .hlsrpack.
function rememberBackup(options, gameId, backup) {
  if (!options.backupsFile) return;
  const all = Object.assign({}, readJsonSafe(options.backupsFile));
  all[gameId] = { stamp: path.basename(backup.dir), sha256: backup.sha256 };
  writeJsonAtomic(options.backupsFile, all);
}

// Путь из import.json от base: без «..», абсолютных путей, «:» и ссылок по дороге.
function plainPath(base, relative, fs) {
  const parts = typeof relative === "string" ? relative.split(/[\\/]/) : [];
  if (!parts.length || path.isAbsolute(relative) || parts.some((part) => !part || part === "." || part === ".." || part.includes(":")))
    throw new DeliveryError("PATH_INVALID", `Недопустимый путь в копии: ${relative}`);
  for (let i = 1; i <= parts.length; i++) {
    let stat = null;
    try { stat = fs.lstatSync(path.join(base, ...parts.slice(0, i))); } catch (error) { /* ещё нет */ }
    if (stat && stat.isSymbolicLink()) throw new DeliveryError("PATH_INVALID", `Ссылка в пути копии: ${relative}`);
  }
  return assertInside(base, path.join(base, ...parts), "Файл копии");
}

// Папка копии .hlsr/backup/<ГГГГММДД-ЧЧММСС>/. Занятое имя (два импорта за секунду) сдвигается.
function newBackupDir(installPath, options, fs) {
  const { stamp, BACKUP_DIR } = require("./config-migration");
  const base = options.now === undefined ? Date.now() : options.now;
  for (let shift = 0; ; shift += 1000) {
    const dir = path.join(installPath, BACKUP_DIR, stamp(base + shift));
    if (!fs.existsSync(dir)) return dir;
  }
}

// Копия файлов перед заменой в новой папке .hlsr/backup/<время>/ и список import.json
// { kept, created } (пути от папки игры): файла нет — он в created, «Вернуть прежний» его удалит.
// Список пишется раньше замены файлов: сбой на полпути возвращается «Вернуть прежний».
function backupFiles(installPath, files, options, fs) {
  const dir = newBackupDir(installPath, options, fs);
  const kept = [];
  const created = [];
  for (const file of files) {
    const relative = path.relative(installPath, file);
    const bytes = readBytes(file, fs);
    if (bytes === null) {
      created.push(relative);
      continue;
    }
    const copy = path.join(dir, relative);
    fs.mkdirSync(path.dirname(copy), { recursive: true });
    fs.writeFileSync(copy, bytes);
    kept.push(relative);
  }
  fs.mkdirSync(dir, { recursive: true });
  const manifest = JSON.stringify({ kept, created }, null, 2);
  fs.writeFileSync(path.join(dir, IMPORT_MANIFEST), manifest);
  return { dir, sha256: sha256Of(manifest) };
}

// Импорт одним файлом (flattenImport): прежний файл игрока и заменяемые файлы
// скриптов уходят в .hlsr/backup/<время>/ (пути от папки игры), потом файлы
// скриптов ложатся в папку мода, и последним — новый файл игрока. expectedSha256
// — отпечаток файла, как его видело окно: изменён за это время — CONFIG_CHANGED.
function importPlayerConfig(installPath, gameId, gameDirectory, built, options = {}) {
  const fs = options.fs || nodeFs;
  const found = playerConfig.locate(installPath, gameId, gameDirectory, options);
  const runtime = Array.isArray(built && built.runtimeFiles) ? built.runtimeFiles : null;
  if (!found) return { status: "unsupported" };
  const names = runtime ? runtime.map((item) => String(item && item.file).toLowerCase()) : [];
  if (!built || typeof built.text !== "string" || !built.text.trim() || !runtime || new Set(names).size !== names.length ||
      !runtime.every((item) => item && typeof item.text === "string" && cfgImport.isSafeImportName(item.file) &&
        !cfgImport.ENGINE_RUN.test(item.file.split("/").pop())) ||
      Buffer.byteLength(built.text, "latin1") + runtime.reduce((sum, item) => sum + Buffer.byteLength(item.text, "latin1"), 0) >
        gameBinds.MAX_IMPORT_BYTES)
    return { status: "invalid" };

  const files = cfgSearch.filesOf(installPath, gameDirectory, found.engine, { ...options, fs });
  const current = playerConfig.read(installPath, gameId, gameDirectory, { ...options, fs });
  if (options.expectedSha256 !== undefined && options.expectedSha256 !== current.sha256)
    return { status: "changed" };
  // HL1 (rulings R12, R16): перед импортом — разделы шаблона «правила» (exec WON.cfg, если импорт
  // его не выполняет сам) и «настройки» (alias run, bind p, строки правил speedrun.com); что импорт
  // задаёт сам, ниже перекроет. Заголовка «Your own» нет: разделы лаунчера при следующих записях
  // дописываются в конец, после импорта.
  const won = cfgImport.execsOf(built.text, found.engine).some((e) => !e.runtime && e.key === "won.cfg");
  const head = gameId === "hl1-won" ? playerConfig.templateLines(gameId, won ? ["settings"] : ["rules", "settings"]) : [];
  const rules = head.length ? `${head.join("\n")}\n\n` : "";
  const crlf = found.encoding === "latin1" || current.text.includes("\r\n");
  const withEol = (text) => (crlf ? text.replace(/\r?\n/g, "\r\n") : text.replace(/\r\n/g, "\n"));

  const targets = runtime.map((item) => ({ item, file: importTarget(files, item.file.replace(/\\/g, "/"), fs) }));
  const backup = backupFiles(installPath, [current.file, ...targets.map(({ file }) => file)], options, fs);
  rememberBackup(options, gameId, backup);
  const relative = (file) => path.relative(installPath, file);

  // Тексты импорта — байты выбранных файлов как latin1 (pickBindsConfig): пишутся теми же байтами (R17).
  for (const { item, file } of targets) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, Buffer.from(withEol(item.text), "latin1"));
  }
  const written = playerConfig.write(installPath, gameId, gameDirectory, withEol(rules + built.text),
    { ...options, fs, expectedSha256: current.sha256 });
  return { status: "ok", sha256: written.sha256, file: written.file, backup: relative(backup.dir), files: targets.map(({ item }) => item.file) };
}

// «Вернуть прежний»: последняя копия .hlsr/backup/<время>/ возвращается байт в байт,
// созданные импортом файлы удаляются. Сначала текущие файлы уходят в новую копию (R18):
// правки после импорта не теряются, и возврат можно вернуть. Использованная копия
// переименовывается в <время>.undone, а не стирается. Берётся только копия, которую
// записал сам лаунчер (options.backupsFile, совпадает SHA-256 её import.json).
function undoImport(installPath, gameId, gameDirectory, options = {}) {
  const fs = options.fs || nodeFs;
  const { BACKUP_DIR } = require("./config-migration");
  const known = options.backupsFile ? Object.assign({}, readJsonSafe(options.backupsFile))[gameId] : null;
  if (!known || !BACKUP_STAMP.test(String(known.stamp))) return { status: "nothing" };
  const dir = path.join(installPath, BACKUP_DIR, known.stamp);
  const bytes = readBytes(path.join(dir, IMPORT_MANIFEST), fs);
  if (bytes === null || sha256Of(bytes) !== known.sha256) return { status: "nothing" };
  const manifest = JSON.parse(bytes.toString("utf8"));
  if (!Array.isArray(manifest.kept) || !Array.isArray(manifest.created))
    throw new DeliveryError("PATH_INVALID", "Список копии повреждён");
  const kept = manifest.kept.map((relative) => [plainPath(dir, relative, fs), plainPath(installPath, relative, fs)]);
  const created = manifest.created.map((relative) => plainPath(installPath, relative, fs));
  const next = backupFiles(installPath, [...kept.map(([, target]) => target), ...created], options, fs);
  for (const [source, target] of kept) {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.copyFileSync(source, target);
  }
  for (const target of created) fs.rmSync(target, { force: true });
  fs.renameSync(dir, `${dir}.undone`);
  rememberBackup(options, gameId, next);
  const read = playerConfig.read(installPath, gameId, gameDirectory, { ...options, fs });
  return { status: "ok", sha256: read.sha256, backup: path.relative(installPath, dir) };
}

// Проверка по правилам speedrun.com (только HL1): файл игрока и все cfg папки мода, которые он может
// выполнить (exec на верхнем уровне и внутри alias и bind), как latin1. Файла игрока нет — "nothing".
// path у находки — путь файла от папки игры, как его знает редактор cfg (GameCfgFiles).
function checkPlayerConfig(installPath, gameId, gameDirectory, options = {}) {
  const fs = options.fs || nodeFs;
  if (gameId !== "hl1-won") return { status: "unsupported" };
  const found = playerConfig.locate(installPath, gameId, gameDirectory, options);
  if (!found) return { status: "unsupported" };
  const current = playerConfig.read(installPath, gameId, gameDirectory, { ...options, fs });
  if (!current.exists) return { status: "nothing" };
  const files = cfgSearch.filesOf(installPath, gameDirectory, found.engine, { ...options, fs });
  const relative = (file) => path.relative(installPath, file).split(path.sep).join("/");
  const root = path.basename(current.file).toLowerCase();
  const texts = { [root]: current.text };
  const paths = { [root]: relative(current.file) };
  const queue = [current.text];
  while (queue.length && Object.keys(texts).length < cfgImport.MAX_FILES) {
    for (const target of cfgImport.execsOf(queue.shift(), found.engine)) {
      if (target.key in texts || !cfgImport.isSafeImportName(target.name)) continue;
      try {
        const file = importTarget(files, target.name, fs);
        if (!fs.statSync(file).isFile() || fs.statSync(file).size > 1024 * 1024) continue;
        texts[target.key] = fs.readFileSync(file).toString("latin1");
        paths[target.key] = relative(file);
        queue.push(texts[target.key]);
      } catch (error) { /* файла нет — проверка покажет «файл не найден» */ }
    }
  }
  const { findings, stats } = checkConfig({ files: texts, root, category: options.category });
  return {
    status: "ok",
    file: paths[root],
    stats,
    findings: findings.map((item) => Object.assign({ path: paths[item.file] || null }, item)),
  };
}

// Экспорт: файл игрока, который зовёт из alias и bind файлы папки мода, уходит
// в выбранную папку вместе с ними (подпапки сохраняются), иначе — один .cfg в
// выбранное место. Байты не меняются. chooseDirectory() и chooseFile(имя) —
// окна выбора (возвращают путь или null), их подставляет вызывающий.
async function exportPlayerConfig(installPath, gameId, gameDirectory, options = {}) {
  const fs = options.fs || nodeFs;
  const found = playerConfig.locate(installPath, gameId, gameDirectory, options);
  if (!found) return { status: "unsupported" };
  const current = playerConfig.read(installPath, gameId, gameDirectory, { ...options, fs });
  if (!current.exists) return { status: "nothing" };
  const files = cfgSearch.filesOf(installPath, gameDirectory, found.engine, { ...options, fs });
  const chain = [];
  const seen = new Set();
  const queue = [current.text];
  while (queue.length && chain.length < cfgImport.MAX_FILES) {
    for (const target of cfgImport.execsOf(queue.shift(), found.engine)) {
      if (!target.runtime || seen.has(target.key) || cfgImport.ENGINE_RUN.test(target.key.split("/").pop()) ||
          !cfgImport.isSafeImportName(target.name)) continue;
      seen.add(target.key);
      const file = importTarget(files, target.name, fs);
      let bytes = null;
      try { if (fs.statSync(file).isFile()) bytes = fs.readFileSync(file); } catch (error) { /* файла нет — ничего не копируем */ }
      if (bytes === null) continue;
      chain.push({ name: target.name, bytes });
      queue.push(bytes.toString(found.encoding));
    }
  }
  const baseName = path.basename(current.file);
  if (!chain.length) {
    const target = await options.chooseFile(`${gameId}-${baseName}`);
    if (!target) return { status: "canceled" };
    fs.writeFileSync(target, fs.readFileSync(current.file));
    return { status: "ok", path: target, files: 1 };
  }
  const chosen = await options.chooseDirectory();
  if (!chosen) return { status: "canceled" };
  // Ничего существующего не перезаписываем: всё ложится в новую подпапку выбранной папки.
  const { stamp } = require("./config-migration");
  const base = options.now === undefined ? Date.now() : options.now;
  const name = `${gameId}-config-${stamp(base)}`;
  let dir = path.join(chosen, name);
  for (let n = 2; fs.existsSync(dir); n += 1) dir = path.join(chosen, `${name}-${n}`);
  fs.mkdirSync(dir, { recursive: true });
  const put = (name, bytes) => {
    const target = assertInside(dir, path.join(dir, name.split("/").join(path.sep)), "Файл экспорта");
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, bytes);
  };
  put(baseName, fs.readFileSync(current.file));
  for (const item of chain) put(item.name, item.bytes);
  return { status: "ok", path: dir, files: chain.length + 1 };
}

module.exports = {
  readImportRecord,
  importPlayerConfig,
  undoImport,
  exportPlayerConfig,
  checkPlayerConfig,
  IMPORT_TRAILER_MARK,
  IMPORT_RECORD,
  importTarget,
  readInstallBinds,
  scriptAliases,
  applyBinds,
  ensureExecLine,
};
