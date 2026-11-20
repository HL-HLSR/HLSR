"use strict";

// Приёмка, Linux (база ctx.base, набор копий ctx.set — как в accept-windows.js): канал →
// LauncherUpdateGate с локального HTTP (qaHttpGet) → вшитый архив →
// ArchiveInstall.prepare (размер, sha256, sha512, строгий USTAR, verifyStage) → commit во временный
// корень → запуск <версия>/hlsr --no-sandbox под headless.sh.
//
// Полностью — на Linux с настоящим рантаймом AppImage: он сам достаёт resources/payload
// (--appimage-extract) без FUSE. На другой ОС рантайм не запустить: вшитый архив берётся из
// payload.json и архива рядом с AppImage (итог `hlsr-release archive`), а шаги рантайма, commit и
// запуска пропускаются с причиной.
//
// Каталог выпуска: HLSR-Setup-<v>-linux-x86_64.AppImage, latest-linux.yml, launcher-channel.json;
// payload.json и HLSR-<v>-linux-x86_64.tar.gz рядом (по желанию на Linux).

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { loadAs } = require("./load-as");
const { expectedLatestYml, sha256Hex, readJson } = require("./accept-windows");

const L = "linux";

// Канал, gate и адреса — как на Linux (платформа подменена: gate читает process.platform).
// Установка и вшитый архив работают с настоящей файловой системой этой машины, поэтому грузятся
// с её путями: ArchiveInstall не читает process.platform, а posix-путь поверх пути Windows ломает
// каталоги. Символические ссылки, /proc и рантайм AppImage есть только на Linux.
function modules(set) {
  return {
    channel: loadAs("main/launcher-update/launcher-channel.js", L, set),
    Gate: loadAs("main/launcher-update/LauncherUpdateGate.js", L, set),
    endpoints: loadAs("shared/delivery-endpoints.js", L, set),
    embedded: loadAs("main/launcher-update/embedded-payload.js", process.platform, set),
    Install: loadAs("main/launcher-update/ArchiveInstall.js", process.platform, set),
  };
}

/** Локальный HTTP на 127.0.0.1: имя → байты (подменяются по ходу проверки). */
function serve(files) {
  return new Promise((resolve) => {
    const server = http.createServer((request, response) => {
      const name = decodeURIComponent(request.url.split("?")[0]).replace(/^\/+/, "");
      if (!files.has(name)) return response.writeHead(404).end("not found");
      const body = files.get(name);
      response.writeHead(200, { "content-length": body.length });
      return response.end(body);
    });
    server.listen(0, "127.0.0.1", () => resolve({ server, base: `http://127.0.0.1:${server.address().port}` }));
  });
}

async function acceptLinux(ctx, report) {
  const m = modules(ctx.set);
  const CURRENT = ctx.base;
  const dir = ctx.dir;
  report.begin(`Linux, база ${CURRENT} (набор ${ctx.set}): ${dir}`);

  let manifest = null;
  await report.step("канал: launcher-channel.json читается", () => {
    manifest = readJson(path.join(dir, "launcher-channel.json"));
    return `версия ${manifest.version}`;
  });
  if (!manifest) return;
  const version = ctx.version || manifest.version;
  const imageName = `HLSR-Setup-${version}-linux-x86_64.AppImage`;
  const image = path.join(dir, imageName);
  const publicKeys = ctx.publicKeys || undefined;
  const checkOptions = { currentVersion: CURRENT, publicKeys, platform: L };

  await report.step(`канал: форма и подпись (launcher-channel.js ${ctx.set})`, () => {
    const shape = m.channel.validateShape(manifest, L);
    if (shape.length) throw new Error(`форма: ${shape.join("; ")}`);
    const verdict = m.channel.check(manifest, checkOptions);
    if (!verdict.ok) throw new Error(verdict.problems.join("; "));
    if (!verdict.updateAvailable) throw new Error(`канал ${manifest.version} не новее ${CURRENT}`);
    return `подписано ключом ${manifest.signature.keyId}`;
  });

  await report.step("версия выпуска — только X.Y.Z (Р9)", () => {
    if (!/^\d+\.\d+\.\d+$/.test(manifest.version)) throw new Error(`версия ${manifest.version}`);
    if (manifest.version !== version) throw new Error(`канал ${manifest.version}, ожидалась ${version}`);
  });

  await report.step("установщик: имя, размер, ровно два адреса", () => {
    const entry = manifest.installer;
    if (entry.filename !== imageName) throw new Error(`канал описывает ${entry.filename}, ожидался ${imageName}`);
    if (!fs.existsSync(image)) throw new Error(`нет файла ${image}`);
    const size = fs.statSync(image).size;
    if (size !== entry.size) throw new Error(`размер ${size} != ${entry.size} из канала`);
    if (size > m.channel.MAX_INSTALLER_BYTES) throw new Error(`${size} байт больше 512 МиБ`);
    const urls = m.endpoints.launcherFor(L).installerUrls(version, imageName);
    if (JSON.stringify(entry.urls) !== JSON.stringify(urls)) throw new Error(`urls ${JSON.stringify(entry.urls)} != ${JSON.stringify(urls)}`);
    return `${(size / 1048576).toFixed(1)} MiB`;
  });

  await report.step("установщик: размер, sha256 и sha512 (verifyInstallerStream)", async () => {
    const result = await m.channel.verifyInstallerStream(image, manifest.installer);
    if (!result.ok) throw new Error(result.problems.join("; "));
  });

  const ymlFile = path.join(dir, "latest-linux.yml");
  await report.step("latest-linux.yml: байт в байт (§2.6) и latestYmlSha256", () => {
    const bytes = fs.readFileSync(ymlFile);
    const text = bytes.toString("utf8");
    const date = /releaseDate: '([^']*)'\n$/.exec(text);
    if (!date) throw new Error("нет releaseDate в конце файла");
    const expected = expectedLatestYml({
      version, filename: imageName, sha512: manifest.installer.sha512, size: manifest.installer.size, releaseDate: date[1],
    });
    if (text !== expected) throw new Error(`текст отличается от шаблона ${ctx.set}:\n${text}`);
    if (sha256Hex(bytes) !== manifest.latestYmlSha256) throw new Error("sha256 latest-linux.yml не совпал с latestYmlSha256 канала");
  });

  await report.step("LauncherUpdateGate.evaluate с локального HTTP (qaHttpGet)", async () => {
    const files = new Map([
      ["launcher-channel.json", fs.readFileSync(path.join(dir, "launcher-channel.json"))],
      ["latest-linux.yml", fs.readFileSync(ymlFile)],
    ]);
    const { server, base } = await serve(files);
    try {
      const gate = () => new m.Gate({
        currentVersion: CURRENT, publicKeys,
        channelUrls: [`${base}/launcher-channel.json`], latestYmlUrls: [`${base}/latest-linux.yml`],
        httpGet: m.Gate.qaHttpGet,
      });
      const outcome = await gate().evaluate();
      if (!outcome.ok || !outcome.updateAvailable) throw new Error(`gate: ${outcome.problems.join("; ")}`);
      // Подмена latest-linux.yml ловится по хешу из подписанного канала.
      files.set("latest-linux.yml", Buffer.from("version: 0.0.0\n"));
      const swapped = await gate().evaluate();
      if (swapped.ok || !/latest\.yml не подтверждён/.test(swapped.problems.join(";"))) throw new Error("gate принял чужой latest-linux.yml");
      return `предлагает ${outcome.manifest.version}; подмена yml отвергнута`;
    } finally {
      server.close();
    }
  });

  let payload = null;
  const payloadFile = path.join(dir, m.embedded.DESCRIPTOR);
  await report.step("вшитый архив: payload.json рядом с AppImage (embedded-payload.read)", () => {
    if (!fs.existsSync(payloadFile)) return { skip: "нет payload.json рядом (итог hlsr-release archive)" };
    payload = m.embedded.read(dir);
    if (payload.manifest.version !== version) throw new Error(`в payload ${payload.manifest.version}, ожидалась ${version}`);
    return `архив ${path.basename(payload.archive)}`;
  });

  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "hlsr-accept-linux-"));
  try {
    await report.step("рантайм AppImage: --appimage-extract resources/payload (extractFrom)", async () => {
      if (process.platform !== "linux") return { skip: "рантайм AppImage запускается только на Linux" };
      const copy = path.join(temporary, imageName);
      fs.copyFileSync(image, copy);
      const extracted = await m.embedded.extractFrom(copy, temporary);
      if (extracted.manifest.version !== version) throw new Error(`в AppImage вшит ${extracted.manifest.version}, ожидалась ${version}`);
      if (payload && JSON.stringify(extracted.manifest) !== JSON.stringify(payload.manifest))
        throw new Error("описание в AppImage отличается от payload.json рядом");
      payload = extracted;
      return `версия ${extracted.manifest.version}, архив ${path.basename(extracted.archive)}`;
    });

    let committed = null;
    const root = path.join(temporary, "install with spaces");
    await report.step("ArchiveInstall.prepare: размер, sha256, sha512, строгий USTAR, verifyStage", async () => {
      if (!payload) return { skip: "нет вшитого архива (ни payload.json рядом, ни рантайма AppImage)" };
      const install = new m.Install(root);
      try {
        await install.prepare(payload.archive, payload.manifest);
        if (process.platform !== "linux") {
          install.discard();
          return "подготовлено и проверено; commit — только Linux (символические ссылки, /proc)";
        }
        committed = install.commit();
        return `версия в ${path.relative(root, path.dirname(committed))}`;
      } catch (error) {
        install.discard();
        throw error;
      }
    });

    await report.step("ArchiveInstall.commit: current, managedRoot, hlsr с правом исполнения", () => {
      if (process.platform !== "linux") return { skip: "ссылки current/previous и /proc — только Linux" };
      if (!committed) throw new Error("commit не выполнен");
      const current = m.Install.readCurrent(root);
      if (!/^versions\/\d+\.\d+\.\d+-[a-f0-9]{16}$/.test(current)) throw new Error(`current: ${current}`);
      if (m.Install.managedRoot(path.join(root, "current", "hlsr")) !== root) throw new Error("managedRoot не узнал корень");
      if ((fs.statSync(committed).mode & 0o111) === 0) throw new Error("hlsr без права исполнения");
      const release = readJson(path.join(path.dirname(committed), ".hlsr-release.json"));
      if (release.version !== version) throw new Error(`.hlsr-release.json: ${release.version}`);
    });

    await report.step("запуск <версия>/hlsr --no-sandbox под headless.sh (HLSR_SMOKE_SNAPSHOT)", () => {
      if (!ctx.smoke) return { skip: "не запрошен (--smoke): нужна сборка с dev-hooks и Linux" };
      if (process.platform !== "linux" || !committed) return { skip: "только Linux после commit" };
      const headless = path.resolve(__dirname, "..", "..", "..", "scripts", "slint", "headless.sh");
      const png = path.join(temporary, "smoke.png");
      const run = spawnSync("bash", [headless, committed, "--no-sandbox"], {
        encoding: "utf8", timeout: 180000,
        env: { ...process.env, HLSR_SMOKE_SNAPSHOT: png, HLSR_DATA_DIR: path.join(temporary, "data"), HLSR_LOGS_DIR: path.join(temporary, "logs") },
      });
      if (run.status !== 0 || !fs.existsSync(png)) throw new Error(`код ${run.status}: ${(run.stderr || "").slice(-300)}`);
      return `снимок ${fs.statSync(png).size} байт`;
    });
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }

  return { manifest, version, image };
}

module.exports = { acceptLinux, modules, serve, crypto };
