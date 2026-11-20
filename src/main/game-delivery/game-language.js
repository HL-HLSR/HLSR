"use strict";

const fs = require("fs");
const path = require("path");
const { execFileSync } = require("child_process");

const { resolveInside } = require("./file-ops");

const AUTO = "auto";
const ENGLISH = "english";

const OVERLAY_DIR = "hlsr-language";

const README =
  "Этой папкой распоряжается лаунчер HLSR.\r\n" +
  "\r\n" +
  "Здесь лежат языковые файлы выбранного в лаунчере языка игры. Папка\r\n" +
  "переписывается при каждом запуске и удаляется, когда язык переключён\r\n" +
  "обратно на «как в Steam». Свои файлы сюда класть не нужно — положи их\r\n" +
  "в соседнюю папку внутри custom/.\r\n";

const FILE_SETS = [
  { base: "gameui", sources: ["mod", "hl2"] },
  { base: "hl2", sources: ["hl2"] },
  { base: "valve", sources: ["hl2"] },
  { base: "chat", sources: ["hl2"] },
];

const REGISTRY_KEY = "HKCU\\Software\\Valve\\Source";
const REGISTRY_VALUE = "Language";

const LANGUAGE_RE = /^[a-z]{2,24}$/;

function isLanguage(value) {
  return typeof value === "string" && LANGUAGE_RE.test(value);
}

function normalizeChoice(value) {
  return isLanguage(value) && value !== AUTO ? value : AUTO;
}

function engineLanguage(options) {
  const opts = options || {};
  const platform = opts.platform !== undefined ? opts.platform : process.platform;

  // Под Proton языка Steam в реестре нет: движок берёт язык по умолчанию.
  if (platform === "linux") return isLanguage(opts.defaultLanguage) ? opts.defaultLanguage : ENGLISH;
  if (platform !== "win32") return null;

  const run = opts.run || execFileSync;

  let output;
  try {
    output = run("reg.exe", ["query", REGISTRY_KEY, "/v", REGISTRY_VALUE], {
      timeout: 3000,
      windowsHide: true,
      encoding: "utf8",
    });
  } catch (error) {
    return null;
  }

  const match = String(output || "").match(/REG_SZ\s+(\S+)/);
  if (!match) return null;

  const value = match[1].trim().toLowerCase();
  return isLanguage(value) ? value : null;
}

function sourceDirs(installPath, gameDirectory) {
  return {
    mod: path.join(installPath, gameDirectory, "resource"),
    hl2: path.join(installPath, "hl2", "resource"),
  };
}

function resourceFileName(base, language) {
  return `${base}_${language}.txt`;
}

function sourceOfChosenLanguage(dirs, set, chosenLanguage, io) {
  for (const key of set.sources) {
    const dir = dirs[key];
    if (!dir) continue;
    const file = path.join(dir, resourceFileName(set.base, chosenLanguage));
    if (io.existsSync(file)) return file;
  }
  return null;
}

function nameEngineWillLookFor(set, languageEngineWillExpand) {
  return resourceFileName(set.base, languageEngineWillExpand);
}

function mountsCustom(installPath, gameDirectory, io) {
  for (const name of ["gameinfo.txt", "GameInfo.txt"]) {
    const file = path.join(installPath, gameDirectory, name);
    let text;
    try {
      text = io.readFileSync(file, "utf8");
    } catch (error) {
      continue;
    }
    if (/^[^/\n]*\bgame\S*\s+\S*custom\/\*/im.test(text)) return true;
  }
  return false;
}

function availableLanguages(installPath, gameDirectory, options) {
  const io = (options && options.fs) || fs;

  if (!mountsCustom(installPath, gameDirectory, io)) return [];

  const dir = sourceDirs(installPath, gameDirectory).mod;

  let names;
  try {
    names = io.readdirSync(dir);
  } catch (error) {
    return [];
  }

  const found = [];
  for (const name of names) {
    const match = /^gameui_([a-z]{2,24})\.txt$/i.exec(String(name));
    if (!match) continue;
    const language = match[1].toLowerCase();
    if (found.indexOf(language) === -1) found.push(language);
  }

  found.sort();
  const rest = found.filter((item) => item !== ENGLISH);
  return found.indexOf(ENGLISH) === -1 ? rest : [ENGLISH].concat(rest);
}

function overlayRoot(installPath, gameDirectory) {
  return resolveInside(
    installPath,
    path.join(gameDirectory, "custom", OVERLAY_DIR),
    "Папка подмены языка"
  );
}

function clear(installPath, gameDirectory, options) {
  const io = (options && options.fs) || fs;
  const root = overlayRoot(installPath, gameDirectory);

  try {
    io.rmSync(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 60 });
  } catch (error) {
    return { ok: false, removed: false, message: error.message };
  }

  return { ok: true, removed: true };
}

function noOverlay(language, engine, reason, cleared) {
  return {
    ok: cleared.ok,
    applied: false,
    language,
    engineLanguage: engine,
    reason: cleared.ok ? reason : "REMOVE_FAILED",
    message: cleared.message,
    files: [],
  };
}

function apply(installPath, gameDirectory, choice, options) {
  const opts = options || {};
  const io = opts.fs || fs;
  const language = normalizeChoice(choice);

  if (language === AUTO)
    return noOverlay(language, null, "AUTO", clear(installPath, gameDirectory, opts));

  const engine = opts.engineLanguage !== undefined ? opts.engineLanguage : engineLanguage(opts);

  if (!isLanguage(engine))
    return noOverlay(language, null, "ENGINE_UNKNOWN", clear(installPath, gameDirectory, opts));

  if (engine === language)
    return noOverlay(language, engine, "SAME_AS_ENGINE", clear(installPath, gameDirectory, opts));

  const dirs = sourceDirs(installPath, gameDirectory);
  const planned = [];

  for (const set of FILE_SETS) {
    const chosenLanguageFile = sourceOfChosenLanguage(dirs, set, language, io);
    if (!chosenLanguageFile) continue;
    planned.push({
      chosenLanguageFile,
      nameEngineWillLookFor: nameEngineWillLookFor(set, engine),
    });
  }

  if (!planned.length)
    return noOverlay(language, engine, "NO_SOURCE", clear(installPath, gameDirectory, opts));

  const cleared = clear(installPath, gameDirectory, opts);
  if (!cleared.ok) return noOverlay(language, engine, "REMOVE_FAILED", cleared);

  const root = overlayRoot(installPath, gameDirectory);
  const resourceDir = path.join(root, "resource");

  try {
    io.mkdirSync(resourceDir, { recursive: true });
    io.writeFileSync(path.join(root, "README.txt"), README, "utf8");

    for (const item of planned)
      io.copyFileSync(item.chosenLanguageFile, path.join(resourceDir, item.nameEngineWillLookFor));
  } catch (error) {
    const cleaned = clear(installPath, gameDirectory, opts);
    return {
      ok: false,
      applied: false,
      language,
      engineLanguage: engine,
      reason: cleaned.ok ? "WRITE_FAILED" : "WRITE_FAILED_DIRTY",
      message: error.message,
      files: [],
    };
  }

  return {
    ok: true,
    applied: true,
    language,
    engineLanguage: engine,
    files: planned.map((item) => item.nameEngineWillLookFor),
  };
}

module.exports = {
  AUTO,
  ENGLISH,
  OVERLAY_DIR,
  FILE_SETS,
  REGISTRY_KEY,
  REGISTRY_VALUE,
  isLanguage,
  normalizeChoice,
  engineLanguage,
  availableLanguages,
  overlayRoot,
  apply,
  clear,
};
