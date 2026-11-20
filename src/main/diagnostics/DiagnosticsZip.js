"use strict";

const fs = require("fs");
const os = require("os");
const path = require("path");
const crypto = require("crypto");
const { spawnSync } = require("child_process");

const exporter = require("./DiagnosticsExport");
const redact = require("./redact");
const discordDiagnostics = require("../discord-rpc/diagnostics-model");
const crashDumps = require("../game-delivery/crash-dumps");

function timestampName(date) {
  const d = date || new Date();
  const pad = (n) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}` +
    `-${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`
  );
}

function sevenZipPath(repoResolve) {
  return require("../seven-zip").binPath();
}

function sha256File(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function buildZip(options) {
  const opts = options || {};
  if (!opts.outFile) throw new Error("не выбран файл экспорта");

  const temp = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-diag-"));
  const tempReal = fs.realpathSync(temp);

  const redactOptions = {
    userName: opts.context ? opts.context.userName : null,
  };

  try {
    exporter.collect(
      Object.assign({}, opts.context, {
        outDir: tempReal,
        logsDir: opts.logsDir,
        now: new Date().toISOString(),
      })
    );

    const summary =
      JSON.stringify(
        {
          schema: 1,
          generatedAt: new Date().toISOString(),
          appVersion: opts.context ? opts.context.appVersion : null,
          checks: opts.checks || [],
          lastSession: opts.statsSummary || null,
          discordRpc: discordDiagnostics.normalize(opts.discordRpc),
        },
        null,
        2
      ) + "\n";

    fs.writeFileSync(
      path.join(tempReal, "diagnostics-summary.json"),
      redact.redactText(summary, redactOptions),
      "utf8"
    );

    for (const [name, file] of [
      ["notifications.json", opts.notificationsFile],
      ["installed.json", opts.installedFile],
    ])
      if (file && fs.existsSync(file))
        try {
          fs.writeFileSync(
            path.join(tempReal, name),
            redact.redactText(fs.readFileSync(file, "utf8"), redactOptions),
            "utf8"
          );
        } catch (e) {
          /* необязательный файл */
        }

    if (opts.installedFile && fs.existsSync(opts.installedFile))
      try {
        const installed = JSON.parse(fs.readFileSync(opts.installedFile, "utf8"));
        const m = installed && installed.buildMetadata ? installed.buildMetadata : null;
        const summary = {
          schema: 1,
          state: m ? "present" : "legacy",
          buildMetadata: m
            ? {
                schemaVersion: m.schemaVersion,
                gameId: m.gameId,
                version: m.version,
                buildId: m.buildId,
                buildName: m.buildName,
                gitCommit: m.gitCommit,
                builtAt: m.builtAt,
                architecture: m.architecture,
                minimumLauncherVersion: m.minimumLauncherVersion,
              }
            : null,
          verifiedAt: installed ? installed.buildMetadataVerifiedAt || null : null,
          buildMetadataSha256: installed ? installed.buildMetadataSha256 || null : null,
          publishedAt: installed ? installed.publishedAt || null : null,
          installedAt: installed ? installed.installedAt || null : null,
        };
        fs.writeFileSync(
          path.join(tempReal, "build-metadata.json"),
          JSON.stringify(summary, null, 2) + "\n",
          "utf8"
        );
      } catch (e) {
        /* необязательный файл */
      }

    const crash = opts.crashInfo;
    if (crash && crash.dumpPath && fs.existsSync(crash.dumpPath)) {
      const meta = {
        name: path.basename(crash.dumpPath),
        size: fs.statSync(crash.dumpPath).size,
        mtime: fs.statSync(crash.dumpPath).mtime.toISOString(),
        sha256: sha256File(crash.dumpPath),
        exitCode: crash.exitCode == null ? null : crash.exitCode,
        hint: crash.hint || null,
        included: false,
      };

      if (
        opts.includeDump === true &&
        crashDumps.isAllowedDumpPath(crash.dumpPath, crash.installPath)
      ) {
        fs.copyFileSync(crash.dumpPath, path.join(tempReal, meta.name));
        meta.included = true;
      }

      fs.writeFileSync(
        path.join(tempReal, "crash-dump.json"),
        JSON.stringify(meta, null, 2) + "\n",
        "utf8"
      );
    }

    const payloadDir = path.join(tempReal, "payload");
    fs.mkdirSync(payloadDir, { recursive: true });
    for (const name of fs.readdirSync(tempReal))
      if (name !== "payload") fs.renameSync(path.join(tempReal, name), path.join(payloadDir, name));

    const zipTemp = path.join(tempReal, "diagnostics.zip");
    const result = spawnSync(sevenZipPath(), ["a", "-tzip", "-y", zipTemp, path.join(payloadDir, "*")], {
      windowsHide: true,
      timeout: 60000,
    });
    if (result.status !== 0 || !fs.existsSync(zipTemp))
      throw new Error("не удалось создать архив диагностики");

    const outTmp = `${opts.outFile}.tmp`;
    fs.copyFileSync(zipTemp, outTmp);
    fs.renameSync(outTmp, opts.outFile);

    return { ok: true, path: opts.outFile };
  } finally {
    try {
      fs.rmSync(tempReal, { recursive: true, force: true });
    } catch (e) {
      /* temp почистит ОС */
    }
  }
}

module.exports = { buildZip, timestampName };
