
const rendererBridge =
  typeof window !== "undefined" && window.hlsrElectron ? window.hlsrElectron.store : null;

let electron = null;
let path = null;
let fs = null;

if (!rendererBridge) {
  electron = require("electron");
  path = require("path");
  fs = require("fs");
}

const BUSY_CODES = ["EBUSY", "EPERM", "EACCES", "EMFILE", "ENFILE", "EAGAIN"];

const READ_ATTEMPTS = 4;
const RETRY_PAUSE_MS = 40;

function isBusy(error) {
  return !!error && BUSY_CODES.indexOf(error.code) !== -1;
}

function pause(ms) {
  const until = Date.now() + ms;
  while (Date.now() < until) continue;
}

function copyDefaults(defaults) {
  if (!defaults || typeof defaults !== "object") return {};

  try {
    return JSON.parse(JSON.stringify(defaults));
  } catch (error) {
    return Object.assign({}, defaults);
  }
}

/**
 * Состояние файла настроек.
 *
 * Возвращает { found, valid, data, reason }:
 *   found:false                  — файла нет, умолчания уместны;
 *   found:true, valid:true       — прочитан и разобран;
 *   found:true, valid:false      — файл ЕСТЬ, но пользоваться им нельзя.
 *     reason "parse" — содержимое не JSON, чинится карантином;
 *     reason "io"    — прочитать не смогли, содержимое неизвестно.
 */
function readState(filePath) {
  let lastError = null;

  for (let attempt = 0; attempt < READ_ATTEMPTS; attempt++) {
    try {
      const raw = fs.readFileSync(filePath, "utf8");

      try {
        return {
          found: true,
          valid: true,
          data: JSON.parse(raw.charCodeAt(0) === 0xfeff ? raw.slice(1) : raw),
        };
      } catch (parseError) {
        return { found: true, valid: false, reason: "parse" };
      }
    } catch (error) {
      lastError = error;

      if (error && error.code === "ENOENT") return { found: false, valid: false };
      if (!isBusy(error)) break;
      if (attempt < READ_ATTEMPTS - 1) pause(RETRY_PAUSE_MS);
    }
  }

  return { found: true, valid: false, reason: "io", code: lastError && lastError.code };
}

function writeState(filePath, data) {
  const temp = `${filePath}.tmp`;
  let handle = null;

  try {
    const text = JSON.stringify(data, null, "\t");

    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    // 0600: в Linux настройки и статистика не видны другим пользователям.
    handle = fs.openSync(temp, "w", 0o600);
    fs.writeFileSync(handle, text);
    fs.fsyncSync(handle);
    fs.closeSync(handle);
    handle = null;

    fs.renameSync(temp, filePath);
    return true;
  } catch (error) {
    if (handle !== null) {
      try {
        fs.closeSync(handle);
      } catch (ignored) {
      }
    }

    try {
      fs.unlinkSync(temp);
    } catch (ignored) {
    }

    return false;
  }
}

function quarantineState(filePath) {
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  const target = `${filePath}.broken-${stamp}`;

  try {
    fs.renameSync(filePath, target);
    return { ok: true, name: path.basename(target) };
  } catch (error) {
    return { ok: false, error: error.message };
  }
}

class Store {
  constructor(opts) {
    this.opts = opts;
    this.bridge = rendererBridge;

    if (!this.bridge) {
      const userDataPath = electron.app.getPath("userData");
      this.path = path.join(userDataPath, opts.configName + ".json");
    }

    this.update();
  }

  update() {
    const state = this.bridge
      ? this.bridge.read(this.opts.configName)
      : readState(this.path);

    if (!state || typeof state !== "object") {
      this.data = copyDefaults(this.opts.defaults);
      this.writable = false;
      this.warn("не получен корректный ответ хранилища");
      return;
    }

    if (state.valid) {
      this.data = state.data;
      this.writable = true;
      return;
    }

    if (!state.found) {
      this.data = copyDefaults(this.opts.defaults);
      this.writable = true;
      return;
    }

    if (state.reason === "parse") {
      this.quarantine();
      this.data = copyDefaults(this.opts.defaults);
      this.writable = true;
      return;
    }

    this.data = copyDefaults(this.opts.defaults);
    this.writable = false;
    this.warn(
      `не прочитан (${state.code || "ошибка ввода-вывода"}); ` +
        "запись отключена до успешного чтения, настройки не тронуты"
    );
  }

  get(key) {
    this.update();
    return this.data[key];
  }

  /**
   * Записать значение.
   *
   * Возвращает true/false вместо исключения: настройки — не та вещь,
   * из-за которой лаунчер должен закрываться. Вызывающий может проверить
   * результат, а может и не проверять — поведение предсказуемо в обоих
   * случаях.
   *
   * false означает и «не смогли записать», и «нечего записывать поверх»:
   * если файл существует, но не прочитан, запись не делается вовсе.
   */
  set(key, val) {
    this.data[key] = val;

    if (this.writable === false) {
      this.warn(`запись "${key}" пропущена: файл есть, но не прочитан`);
      return false;
    }

    return this.bridge
      ? this.bridge.write(this.opts.configName, this.data) === true
      : writeState(this.path, this.data);
  }

  quarantine() {
    const result = this.bridge
      ? this.bridge.quarantine(this.opts.configName)
      : quarantineState(this.path);

    if (result && result.ok)
      this.warn(`не разобран как JSON, отложен: ${result.name}`);
    else
      this.warn(
        `не разобран как JSON, отложить не удалось: ${(result && result.error) || "ошибка"}`
      );
  }

  warn(message) {
    const text = `[store] ${this.opts.configName}.json: ${message}`;

    if (!this.bridge)
      try {
        require("electron-log").warn(text);
      } catch (ignored) {
      }

    console.warn(text);
  }
}

export default Store;
export { readState, writeState, quarantineState, copyDefaults };
