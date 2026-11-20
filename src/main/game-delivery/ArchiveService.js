"use strict";

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

const { parseListing, inspectEntries } = require("./archive-entries");
const { DeliveryError, isAntivirusBlock, antivirusError } = require("./errors");

const LIST_TIMEOUT_MS = 5 * 60 * 1000;

function defaultBinPath() {
  return require("../seven-zip").binPath();
}

class ArchiveService {
  constructor(options) {
    const opts = options || {};
    this.binPath = opts.binPath || defaultBinPath();
    this.logger = opts.logger || { info() {}, warn() {}, error() {} };
  }

  list(archivePath) {
    return new Promise((resolve, reject) => {
      const child = spawn(this.binPath, ["l", "-slt", "-sccUTF-8", "-p", archivePath], {
        windowsHide: true,
      });

      let stdout = "";
      let stderr = "";
      const timer = setTimeout(() => {
        child.kill();
        reject(new DeliveryError("ARCHIVE_LIST_FAILED", "7-Zip не ответил за отведённое время"));
      }, LIST_TIMEOUT_MS);

      child.stdout.setEncoding("utf8");
      child.stderr.setEncoding("utf8");
      child.stdout.on("data", (chunk) => {
        stdout += chunk;
      });
      child.stderr.on("data", (chunk) => {
        stderr += chunk;
      });

      child.on("error", (error) => {
        clearTimeout(timer);
        reject(new DeliveryError("ARCHIVE_LIST_FAILED", `7-Zip не запустился: ${error.message}`));
      });

      child.on("close", (code) => {
        clearTimeout(timer);

        if (code !== 0) {
          reject(
            new DeliveryError(
              "ARCHIVE_LIST_FAILED",
              `7-Zip завершился с кодом ${code}: ${stderr.trim().slice(0, 400)}`
            )
          );
          return;
        }

        resolve(parseListing(stdout));
      });
    });
  }

  async inspect(archivePath, options) {
    const opts = options || {};
    const listing = await this.list(archivePath);

    const report = inspectEntries(listing.entries, {
      maxEntries: opts.maxEntries,
      maxUncompressedSize: opts.maxUncompressedSize,
    });

    if (report.unsafe.length)
      throw new DeliveryError(
        "ARCHIVE_UNSAFE_ENTRY",
        `Архив содержит небезопасные пути: ${report.unsafe[0].path}`,
        { entries: report.unsafe.slice(0, 10) }
      );

    if (report.symlinks.length)
      throw new DeliveryError(
        "ARCHIVE_UNSAFE_ENTRY",
        `Архив содержит symlink/reparse point: ${report.symlinks[0]}`,
        { entries: report.symlinks.slice(0, 10) }
      );

    if (report.encrypted.length)
      throw new DeliveryError(
        "ARCHIVE_UNSAFE_ENTRY",
        `Архив содержит зашифрованные элементы: ${report.encrypted[0]}`
      );

    if (report.errors.indexOf("too-many-entries") !== -1)
      throw new DeliveryError(
        "ARCHIVE_LIMIT_EXCEEDED",
        `В архиве ${report.counts.total} элементов при лимите ${opts.maxEntries}`,
        { limit: opts.maxEntries, actual: report.counts.total }
      );

    if (report.errors.indexOf("uncompressed-size-limit") !== -1)
      throw new DeliveryError(
        "ARCHIVE_LIMIT_EXCEEDED",
        `Распакованный размер ${report.uncompressedSize} превышает лимит ${opts.maxUncompressedSize}`,
        { limit: opts.maxUncompressedSize, actual: report.uncompressedSize }
      );

    if (opts.rootPrefix) {
      const roots = report.roots;
      if (roots.length !== 1 || roots[0] !== opts.rootPrefix)
        throw new DeliveryError(
          "ARCHIVE_UNSAFE_ENTRY",
          `Ожидалась единственная корневая папка "${opts.rootPrefix}", найдено: ${roots.join(", ")}`,
          { roots }
        );
    }

    return { listing, report };
  }

  extract(archivePath, destination, options) {
    const opts = options || {};
    const Seven = require("node-7z");

    let cancelled = false;
    let stream = null;

    const promise = new Promise((resolve, reject) => {
      // opts.files — только эти пути архива, без папок (7z e).
      const only = Array.isArray(opts.files) && opts.files.length ? opts.files : null;
      stream = Seven[only ? "extract" : "extractFull"](archivePath, destination, {
        $bin: this.binPath,
        $progress: true,
        $spawnOptions: { windowsHide: true },
        $raw: ["-snl-"],
        $cherryPick: only || undefined,
      });

      stream.on("progress", (progress) => {
        if (opts.onProgress) opts.onProgress(progress.percent / 100, progress);
      });

      stream.on("error", (error) => {
        if (cancelled) {
          reject(new DeliveryError("CANCELLED", "Распаковка отменена"));
          return;
        }

        const details = (error.stderr || "").trim().slice(0, 400);
        if (isAntivirusBlock(error)) {
          reject(antivirusError(archivePath, error));
          return;
        }
        reject(
          new DeliveryError(
            "EXTRACT_FAILED",
            `Ошибка распаковки: ${details || error.message}`
          )
        );
      });

      stream.on("end", () => {
        if (cancelled) reject(new DeliveryError("CANCELLED", "Распаковка отменена"));
        else resolve({ destination });
      });
    });

    const cancel = () => {
      cancelled = true;
      const child = stream && stream._childProcess;

      if (child && !child.killed) {
        try {
          child.kill();
        } catch (e) {
          this.logger.warn(`[game-delivery] не удалось остановить 7-Zip: ${e.message}`);
        }
      }
    };

    return { promise, cancel };
  }

  // Архив 7z из файлов папки cwd (пути относительно неё). headFiles ложатся
  // в корень отдельным первым блоком: их можно прочитать, не распаковывая
  // остальное (иначе ради мелкого файла пришлось бы разжать гигабайты
  // плотного блока). Максимальное сжатие; потоков не больше четырёх: на
  // -mx=9 каждая пара берёт около 0,7 ГБ памяти.
  create(archivePath, options) {
    const opts = options || {};
    const listFile = path.join(
      opts.listDir || path.dirname(archivePath),
      `.hlsr-pack-${process.pid}-${Date.now().toString(36)}.txt`
    );
    const threads = Math.max(1, Math.min(4, Number(opts.threads) || 1));
    const zip = opts.type === "zip";
    const common = [zip ? "-tzip" : "-t7z", "-snl-", "-scsUTF-8", "-sccUTF-8", "-y"];
    let child = null;
    let cancelled = false;

    const run = (args, onProgress) =>
      new Promise((resolve, reject) => {
        if (cancelled) {
          reject(new DeliveryError("CANCELLED", "Экспорт отменён"));
          return;
        }
        child = spawn(this.binPath, args, { cwd: opts.cwd, windowsHide: true });

        let stderr = "";
        let last = -1;
        child.stdout.setEncoding("utf8");
        child.stderr.setEncoding("utf8");
        child.stdout.on("data", (chunk) => {
          const found = String(chunk).match(/(\d{1,3})%/g);
          if (!found || !onProgress) return;
          const percent = Math.min(100, parseInt(found[found.length - 1], 10));
          if (percent !== last) onProgress((last = percent));
        });
        child.stderr.on("data", (chunk) => {
          if (stderr.length < 4000) stderr += chunk;
        });

        child.on("error", (error) => {
          reject(new DeliveryError("PACK_FAILED", `7-Zip не запустился: ${error.message}`));
        });

        child.on("close", (code) => {
          if (cancelled) reject(new DeliveryError("CANCELLED", "Экспорт отменён"));
          else if (code === 0) resolve();
          else if (isAntivirusBlock({ stderr })) reject(antivirusError(archivePath, { message: stderr.trim() }));
          else reject(new DeliveryError("PACK_FAILED", `7-Zip завершился с кодом ${code}: ${stderr.trim().slice(0, 400)}`));
        });
      });

    const promise = (async () => {
      try {
        fs.writeFileSync(listFile, opts.files.map((file) => file.split("/").join(path.sep)).join("\r\n") + "\r\n", "utf8");
      } catch (error) {
        throw new DeliveryError("PACK_FAILED", `Не удалось записать список файлов: ${error.message}`);
      }
      try {
        if (!zip && opts.headFiles && opts.headFiles.length)
          await run(["a", ...common, "-mx=0", "-bso0", "-bsp0", archivePath].concat(opts.headFiles));
        await run(["a", ...common, zip ? "-mx=5" : "-mx=9", `-mmt=${threads}`, "-bso0", "-bsp1", archivePath, `@${listFile}`], opts.onProgress);
        return { archivePath };
      } finally {
        this.cleanupListFile(listFile);
      }
    })();

    const cancel = () => {
      cancelled = true;
      if (child && !child.killed) {
        try {
          child.kill();
        } catch (e) {
          this.logger.warn(`[game-delivery] не удалось остановить 7-Zip: ${e.message}`);
        }
      }
    };

    return { promise, cancel };
  }

  // Один файл из архива — в строку (служебные .json сборки).
  readText(archivePath, archiveEntry, maxBytes) {
    return new Promise((resolve, reject) => {
      const child = spawn(this.binPath, ["e", "-so", "-sccUTF-8", "-p", archivePath, archiveEntry], {
        windowsHide: true,
      });
      const chunks = [];
      let size = 0;
      let stderr = "";
      let tooLarge = false;
      child.stdout.on("data", (chunk) => {
        size += chunk.length;
        if (size > maxBytes) {
          tooLarge = true;
          child.kill();
          return;
        }
        chunks.push(chunk);
      });
      child.stderr.setEncoding("utf8");
      child.stderr.on("data", (chunk) => {
        if (stderr.length < 4000) stderr += chunk;
      });
      child.on("error", (error) => reject(new DeliveryError("ARCHIVE_LIST_FAILED", `7-Zip не запустился: ${error.message}`)));
      child.on("close", (code) => {
        if (tooLarge) reject(new DeliveryError("ARCHIVE_LIMIT_EXCEEDED", `${archiveEntry} больше ${maxBytes} байт`));
        else if (code !== 0) {
          if (isAntivirusBlock({ stderr })) reject(antivirusError(archivePath, { message: stderr.trim() }));
          else reject(new DeliveryError("ARCHIVE_LIST_FAILED", `7-Zip завершился с кодом ${code}: ${stderr.trim().slice(0, 400)}`));
        } else resolve(Buffer.concat(chunks).toString("utf8"));
      });
    });
  }

  cleanupListFile(listFile) {
    try {
      fs.rmSync(listFile, { force: true });
    } catch (e) {
      this.logger.warn(`[game-delivery] не удалось удалить список файлов: ${e.message}`);
    }
  }

  static walk(root, limit) {
    const max = limit || 500000;
    const result = [];
    const stack = [root];

    while (stack.length) {
      const dir = stack.pop();
      let items;

      try {
        items = fs.readdirSync(dir, { withFileTypes: true });
      } catch (e) {
        continue;
      }

      for (const item of items) {
        const full = path.join(dir, item.name);
        if (item.isDirectory()) stack.push(full);
        else {
          result.push(path.relative(root, full).replace(/\\/g, "/"));
          if (result.length >= max) return result;
        }
      }
    }

    return result;
  }
}

module.exports = ArchiveService;
module.exports.defaultBinPath = defaultBinPath;
