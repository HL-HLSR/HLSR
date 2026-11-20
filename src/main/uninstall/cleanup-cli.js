"use strict";

const fs = require("fs");
const path = require("path");

const cleanup = require("./library-cleanup");

const FLAG = "--hlsr-uninstall-cleanup";

function isCleanupInvocation(argv) {
  return (argv || []).indexOf(FLAG) !== -1;
}

function parseArgs(argv) {
  const args = {
    games: false,
    settings: false,
    keepProfile: false,
    deleteProfile: false,
  };
  const list = (argv || []).slice();

  for (let i = 0; i < list.length; i++) {
    const arg = String(list[i]);

    if (arg === "--games") args.games = true;
    else if (arg === "--purge") args.purge = true;
    else if (arg === "--settings") args.settings = true;
    else if (arg === "--keep-profile") args.keepProfile = true;
    else if (arg === "--delete-profile") args.deleteProfile = true;
    else if (arg === "--print-library") args.printLibrary = true;
    else if (arg === "--library") args.library = list[++i];
    else if (arg.startsWith("--library=")) args.library = arg.slice("--library=".length);
    else if (arg === "--install-dir") args.installDir = list[++i];
    else if (arg.startsWith("--install-dir=")) args.installDir = arg.slice("--install-dir=".length);
    else if (arg === "--report") args.report = list[++i];
    else if (arg.startsWith("--report=")) args.report = arg.slice("--report=".length);
  }

  return args;
}

function libraryPathFrom(userDataDir) {
  try {
    const raw = fs.readFileSync(path.join(userDataDir, "library.json"), "utf8").replace(/^\uFEFF/, "");
    const data = JSON.parse(raw);
    if (data && typeof data.libraryPath === "string" && data.libraryPath.trim())
      return data.libraryPath.trim();
  } catch (ignored) {
  }

  return null;
}

function worst(a, b) {
  if (a === cleanup.CODES.OK || a === cleanup.CODES.NOTHING_TO_DO) return b;
  return a;
}

function reportUninstall(userData, opts, say) {
  try {
    const TelemetryService = require("../telemetry/TelemetryService");

    const telemetry = new TelemetryService({
      userDataDir: userData,
      launcherVersion: opts.launcherVersion || "0.0.0",
      logger: { info() {}, warn() {}, error() {} },
      localeProvider: () => "ru",
      isWindowVisible: () => false,
    });

    if (!telemetry.enabled()) {
      say("telemetry: выключена, событие удаления не создано");
      return;
    }

    if (!telemetry.reportUninstallInitiated()) {
      say("telemetry: событие удаления не поставлено в очередь");
      return;
    }

    say("telemetry: отправлено событие о запуске удаления");

    telemetry.sender.flush().catch(() => {});
  } catch (error) {
    say("telemetry: пропущено");
  }
}

function run(options) {
  const opts = options || {};
  const args = parseArgs(opts.argv || process.argv);
  const lines = [];
  const say = (text) => {
    lines.push(text);
    if (opts.silent !== true) process.stdout.write(text + "\n");
  };

  const userData = opts.userData || null;
  let code = cleanup.CODES.NOTHING_TO_DO;

  if (args.printLibrary) {
    const library = (userData ? libraryPathFrom(userData) : null) || "";

    if (args.report) {
      try {
        fs.writeFileSync(args.report, Buffer.from(library, "utf16le"));
      } catch (error) {
        return { code: cleanup.CODES.INTERNAL, lines: [] };
      }
    }

    if (opts.silent !== true) process.stdout.write(library + "\n");
    return { code: library ? cleanup.CODES.OK : cleanup.CODES.NOTHING_TO_DO, lines: [library] };
  }

  say(
    `HLSR uninstall cleanup: games=${args.games ? "yes" : "no"} ` +
      `purge=${args.purge ? "yes" : "no"} ` +
      `settings=${args.settings ? "yes" : "no"} ` +
      `keepProfile=${args.keepProfile ? "yes" : "no"} ` +
      `deleteProfile=${args.deleteProfile ? "yes" : "no"}`
  );

  if (userData && opts.reportUninstall !== false) reportUninstall(userData, opts, say);

  if (args.games) {
    const library = args.library || (userData ? libraryPathFrom(userData) : null);

    if (!library) {
      say("games: путь библиотеки неизвестен, ничего не удалено");
      code = worst(code, cleanup.CODES.NOTHING_TO_DO);
    } else {
      const result = cleanup.cleanupLibrary(library, {
        installDir: args.installDir || opts.installDir || null,
        purge: args.purge === true,
        env: opts.env || process.env,
        home: opts.home || null,
        steamLibraries: opts.steamLibraries || null,
      });

      if (result.ok) {
        say(
          `games: очищено ${result.removed} объект(ов) в ${result.root}` +
            (result.rootRemoved ? " (корень удалён)" : " (корень сохранён)")
        );
      } else {
        say(`games: ОТКАЗ (${result.code}) ${result.reason || ""} путь=${result.root || library}`);
      }

      code = worst(code, result.code);
    }
  }

  if (args.settings) {
    const result = cleanup.cleanupSettings({
      userData,
      logsDir: opts.logsDir || null,
      updaterCache: opts.updaterCache || null,
      appName: opts.appName || null,
      env: opts.env || process.env,
      home: opts.home || null,
      steamLibraries: opts.steamLibraries || null,
      installDir: args.installDir || opts.installDir || null,
      preserveProfile: args.keepProfile,
    });

    if (result.ok) say(`settings: очищено ${result.removed} объект(ов)`);
    else say(`settings: частичный отказ, не удалено ${result.failed.length} объект(ов)`);

    code = worst(code, result.code);
  } else if (args.deleteProfile) {
    const result = cleanup.cleanupProfile({
      userData,
      env: opts.env || process.env,
      home: opts.home || null,
      installDir: args.installDir || opts.installDir || null,
    });

    if (result.ok) say(`profile: удалено ${result.removed} объект(ов)`);
    else say(`profile: отказ, не удалено ${result.failed.length} объект(ов)`);

    code = worst(code, result.code);
  }

  if (args.report) {
    try {
      fs.writeFileSync(args.report, lines.join("\r\n") + "\r\n", "utf8");
    } catch (ignored) {
    }
  }

  return { code, lines };
}

function runFromMain(app) {
  let code = cleanup.CODES.INTERNAL;

  try {
    const userData = app.getPath("userData");
    const localAppData = process.env.LOCALAPPDATA || "";
    const appName = app.getName();

    const result = run({
      argv: process.argv,
      userData,
      appName,
      launcherVersion: app.getVersion(),
      logsDir: localAppData ? path.join(localAppData, appName, "logs") : null,
      updaterCache: localAppData ? path.join(localAppData, `${appName}-updater`) : null,
      installDir: path.dirname(app.getPath("exe")),
    });

    code = result.code;
  } catch (error) {
    process.stderr.write(`cleanup failed: ${error && error.message}\n`);
    code = cleanup.CODES.INTERNAL;
  }

  app.exit(code);
}

module.exports = { FLAG, isCleanupInvocation, parseArgs, libraryPathFrom, run, runFromMain };
