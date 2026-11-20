"use strict";

const fs = require("fs");

const personalization = require("../game-delivery/ghosting-personalization");

// Чистая логика рендера Source (Ghosting): какие сегменты рендерить, пачки
// для startdemos, таймлайн из плейлиста, разбор журнала консоли, TGA и WAV.
// Процессы и файлы игры — в ghosting.js.

// startdemos принимает не больше 32 имён (MAX_DEMOS движка), а строка команды —
// не длиннее 512 символов (COMMAND_MAX_LENGTH): длиннее движок молча отбрасывает.
const MAX_BATCH = 32;
const MAX_COMMAND = 500;
// Тикрейт Ghosting; заголовок демо даёт его как ticks / playback_time.
const DEFAULT_TICKRATE = 100;
const round3 = (value) => Math.round(value * 1000) / 1000;

// Сегменты в порядке плейлиста со временем демо (как в library.describe:
// сумма длительностей). Рендерятся непустые (ticks > 1: пустые — спам
// сейвлоуда, gh_demo_skip_empty их тоже пропускает), пересекающие интервал.
function planSegments(segments, range, fps) {
  const all = [];
  let t = 0;
  for (let i = 0; i < (segments || []).length; i++) {
    const segment = segments[i];
    const duration = Number(segment.duration) >= 0 ? Number(segment.duration) : 0;
    all.push({ segment, index: i, start: t, end: t + duration, duration });
    t += duration;
  }
  const from = range ? range.from : 0;
  const to = range ? range.to : t;
  const picked = all.filter((item) => (item.segment.ticks || 0) > 1 && item.end > from && item.start < to);
  for (const item of picked) {
    const tickrate = item.segment.tickrate > 0 ? item.segment.tickrate : DEFAULT_TICKRATE;
    item.frames = Math.round((item.segment.ticks / tickrate) * fps);
  }
  if (!picked.length) return { picked, offset: from, cuts: [], framesTotal: 0 };
  // Материал — подряд отрендеренные сегменты: пропущенные между ними
  // (пустые) вырезаны из времени демо.
  const cuts = [];
  const first = picked[0].index;
  const last = picked[picked.length - 1].index;
  for (const item of all)
    if (item.index > first && item.index < last && !picked.includes(item) && item.duration > 0)
      cuts.push({ from: round3(item.start), to: round3(item.end) });
  const framesTotal = picked.reduce((sum, item) => sum + item.frames, 0);
  // Кадров достаточно, когда материал дошёл до конца интервала (+2 кадра
  // запаса): клип из начала длинного сегмента не ждёт его конца.
  let removed = 0;
  for (const cut of cuts) if (cut.to <= to) removed += cut.to - cut.from;
  const needed = Math.ceil((to - picked[0].start - removed) * fps) + 2;
  return {
    picked,
    offset: round3(picked[0].start),
    cuts,
    framesTotal,
    framesNeeded: needed < framesTotal ? Math.max(1, needed) : null,
  };
}

// Сколько имён длиной nameLength влезает в одну команду startdemos.
function batchLimit(nameLength) {
  return Math.max(1, Math.min(MAX_BATCH, Math.floor((MAX_COMMAND - "startdemos".length) / (nameLength + 1))));
}

function batches(list, size) {
  const step = Math.max(1, Math.min(MAX_BATCH, size || MAX_BATCH));
  const out = [];
  for (let i = 0; i < list.length; i += step) out.push(list.slice(i, i + step));
  return out;
}

function clockLabel(seconds) {
  const ms = Math.round(Math.max(0, seconds) * 1000);
  const minutes = Math.floor(ms / 60000);
  const rest = (ms % 60000) / 1000;
  return `${minutes}:${rest.toFixed(3).padStart(6, "0")}`;
}

// Метки во времени демо (jobs.js переводит их во время видео): начало,
// смены карт и сейвлоуды на стыках отрендеренных сегментов, таймер рана на
// стыках и в конце — только то, что есть в плейлисте и заголовках.
function timelineOf(picked, range) {
  const markers = [];
  for (let k = 0; k < picked.length; k++) {
    const item = picked[k];
    const segment = item.segment;
    const map = segment.map || segment.playlistMap || "";
    const at = round3(k === 0 ? Math.max(item.start, range ? range.from : 0) : item.start);
    if (k === 0) markers.push({ t: at, type: "map", label: map });
    else {
      const previous = picked[k - 1].segment;
      const changed = map && map !== (previous.map || previous.playlistMap || "");
      markers.push({ t: at, type: changed ? "map" : "saveload", label: changed ? map : String(item.index + 1) });
    }
    if (Number.isFinite(segment.timerStart) && at === round3(item.start)) markers.push({ t: at, type: "timer", label: clockLabel(segment.timerStart) });
  }
  const last = picked[picked.length - 1];
  if (last && Number.isFinite(last.segment.timerEnd))
    markers.push({ t: round3(last.end), type: "timer", label: clockLabel(last.segment.timerEnd) });
  return markers;
}

// Журнал консоли (+con_logfile, +con_timestamp 0). Движок пишет его построчно
// (открыл, дописал, закрыл), поэтому строки приходят сразу. Шаблоны привязаны
// к началу строк движка: с developer 1 журнал шумный (cubemap, lightmap…).
const LOG_RULES = [
  { re: /^Playing demo from (.+?)\.?\s*$/i, kind: "playing" },
  { re: /^CDemoFile::Open: couldn't open file/i, kind: "error", code: "DEMO_OPEN_FAILED" },
  { re: /^(?:ERROR: )?demo (?:file|network) protocol \d+ outdated/i, kind: "error", code: "DEMO_PROTOCOL_UNSUPPORTED" },
  { re: /^(?:Missing map maps\/\S+\.bsp|Host_Error: Map is missing|Couldn't spawn server|Map load failed)/i, kind: "error", code: "MAP_MISSING" },
  { re: /^Host_Error:/i, kind: "error", code: "GAME_ERROR" },
];
function parseLogLine(line) {
  const text = String(line || "").replace(/\r$/, "");
  for (const rule of LOG_RULES) {
    const match = rule.re.exec(text);
    if (!match) continue;
    if (rule.kind === "playing") return { kind: "playing", demo: demoKey(match[1]) };
    return { kind: "error", code: rule.code, text: text.slice(0, 300) };
  }
  return null;
}

// «hlsr-render/ab12cd34/s001.dem» -> «hlsr-render/ab12cd34/s001».
function demoKey(name) {
  return String(name || "").trim().replace(/\\/g, "/").replace(/\.dem$/i, "").toLowerCase();
}

// Несжатый TGA игры (тип 2, 24/32 бит): размер, формат и где пиксели.
function tgaInfo(buffer) {
  if (!buffer || buffer.length < 18) return null;
  const idLength = buffer[0];
  const width = buffer.readUInt16LE(12);
  const height = buffer.readUInt16LE(14);
  const bpp = buffer[16];
  if (buffer[1] !== 0 || buffer[2] !== 2 || (bpp !== 24 && bpp !== 32) || !width || !height) return null;
  const offset = 18 + idLength;
  const bytes = width * height * (bpp / 8);
  return {
    width,
    height,
    bpp,
    pixFmt: bpp === 24 ? "bgr24" : "bgra",
    topDown: (buffer[17] & 0x20) !== 0,
    offset,
    bytes,
    complete: buffer.length >= offset + bytes,
  };
}

// WAV после TerminateProcess: игра не дошла до endmovie, в RIFF/data
// нулевые размеры (как movie_fixwave). Чинит заголовок по длине файла,
// отрезает недописанный сэмпл. Возвращает формат и длительность.
function wavLayout(buffer, fileSize) {
  if (!buffer || buffer.length < 12 || buffer.toString("latin1", 0, 4) !== "RIFF" || buffer.toString("latin1", 8, 12) !== "WAVE") return null;
  let at = 12;
  let format = null;
  while (at + 8 <= buffer.length) {
    const id = buffer.toString("latin1", at, at + 4);
    const size = buffer.readUInt32LE(at + 4);
    if (id === "fmt " && at + 24 <= buffer.length) {
      format = {
        channels: buffer.readUInt16LE(at + 10),
        rate: buffer.readUInt32LE(at + 12),
        byteRate: buffer.readUInt32LE(at + 16),
        blockAlign: buffer.readUInt16LE(at + 20),
        bits: buffer.readUInt16LE(at + 22),
      };
    }
    if (id === "data") {
      if (!format || !format.blockAlign || !format.byteRate) return null;
      const dataAt = at + 8;
      let length = Math.max(0, fileSize - dataAt);
      length -= length % format.blockAlign;
      return Object.assign({ dataAt, length, duration: length / format.byteRate }, format);
    }
    at += 8 + size + (size & 1);
  }
  return null;
}

function fixWavFile(file) {
  const fd = fs.openSync(file, "r+");
  try {
    const size = fs.fstatSync(fd).size;
    const head = Buffer.alloc(Math.min(size, 4096));
    fs.readSync(fd, head, 0, head.length, 0);
    const layout = wavLayout(head, size);
    if (!layout) return null;
    const riff = Buffer.alloc(4);
    riff.writeUInt32LE(layout.dataAt + layout.length - 8);
    fs.writeSync(fd, riff, 0, 4, 4);
    const data = Buffer.alloc(4);
    data.writeUInt32LE(layout.length);
    fs.writeSync(fd, data, 0, 4, layout.dataAt - 4);
    fs.ftruncateSync(fd, layout.dataAt + layout.length);
    return layout;
  } finally {
    fs.closeSync(fd);
  }
}

function wavHeader(format, dataLength) {
  const header = Buffer.alloc(44);
  header.write("RIFF", 0, "latin1");
  header.writeUInt32LE(36 + dataLength, 4);
  header.write("WAVEfmt ", 8, "latin1");
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);
  header.writeUInt16LE(format.channels, 22);
  header.writeUInt32LE(format.rate, 24);
  header.writeUInt32LE(format.byteRate, 28);
  header.writeUInt16LE(format.blockAlign, 32);
  header.writeUInt16LE(format.bits, 34);
  header.write("data", 36, "latin1");
  header.writeUInt32LE(dataLength, 40);
  return header;
}

// Звук пачек — в один WAV, ровно под видео: у каждой пачки отрезается
// опережение микшера (wavDur − frames/fps, ~0,117 с при 60 к/с) и берётся
// ровно frames/fps секунд (недостающее — тишиной), так что следующая пачка
// начинается точно на своём кадре.
class WavJoiner {
  constructor(file) {
    this.file = file;
    this.format = null;
    this.length = 0;
  }

  // lead — опережение микшера, если его нельзя измерить по длине WAV (игру
  // закрыли посреди сегмента, звук писался и после последнего кадра).
  // Возвращает сдвиг этой пачки (с) — для журнала. Копирование асинхронное:
  // WAV пачки — десятки и сотни МБ, главный процесс не блокируем.
  async append(source, frames, fps, lead) {
    const layout = fixWavFile(source);
    if (!layout || layout.bits !== 16) throw Object.assign(new Error("WAV игры не читается"), { code: "AUDIO_INVALID" });
    if (!this.format) {
      this.format = layout;
      fs.writeFileSync(this.file, wavHeader(layout, 0));
    } else if (layout.rate !== this.format.rate || layout.channels !== this.format.channels)
      throw Object.assign(new Error("Формат звука пачек разный"), { code: "AUDIO_INVALID" });
    const block = this.format.blockAlign;
    const wanted = Math.round((frames / fps) * this.format.rate) * block;
    const offsetBlocks = Math.max(0, Math.round((lead === undefined ? layout.duration - frames / fps : lead) * this.format.rate));
    let skip = offsetBlocks * block;
    if (skip > layout.length) skip = layout.length;
    const take = Math.min(wanted, layout.length - skip);
    const input = await fs.promises.open(source, "r");
    const output = await fs.promises.open(this.file, "r+");
    try {
      const chunk = Buffer.alloc(1024 * 1024);
      let done = 0;
      while (done < take) {
        const { bytesRead } = await input.read(chunk, 0, Math.min(chunk.length, take - done), layout.dataAt + skip + done);
        if (bytesRead <= 0) break;
        await output.write(chunk, 0, bytesRead, 44 + this.length + done);
        done += bytesRead;
      }
      for (let pad = wanted - done; pad > 0;) {
        const size = Math.min(pad, chunk.length);
        await output.write(Buffer.alloc(size), 0, size, 44 + this.length + wanted - pad);
        pad -= size;
      }
    } finally {
      await input.close();
      await output.close();
    }
    this.length += wanted;
    return round3(offsetBlocks / this.format.rate);
  }

  close() {
    if (!this.format) return null;
    const fd = fs.openSync(this.file, "r+");
    try {
      fs.writeSync(fd, wavHeader(this.format, this.length), 0, 44, 0);
    } finally {
      fs.closeSync(fd);
    }
    return this.file;
  }
}

// Командная строка Ghosting. Пути консоли — от папки мода, startdemos
// хранит имя не длиннее 31 символа (MAX_DEMONAME): папка задания короткая.
// Конфиг и HUD игрока в рендере Ghosting. Свой конфиг — как всегда: движок читает cfg/config.cfg
// и cfg/autoexec.cfg игрока, а autoexec зовёт hlsr-personalization.cfg (HUD из «Персонализации»).
// Стандартный — папка задания впереди путей поиска (-insert_search_path) со своими config.cfg и
// autoexec.cfg без настроек. Проверено 11.11 на стенде (1.0.12): движок исполняет их вместо файлов
// игрока, журнал консоли и startmovie пишут по-прежнему в папку мода, остальные cfg мода находятся
// как раньше. Свой HUD при стандартном конфиге — +exec hlsr-personalization.cfg игрока; стандартный
// HUD при своём конфиге — значения «Персонализации» по умолчанию после autoexec (+exec своего cfg).
// HUD целиком в Ghosting выключает только cl_drawhud — чит, движок не даёт задать его до демки,
// а sv_cheats 1 сбросит его на первой же демке: без поддержки в моде настройки нет.
// dir — папка от корня игры (рядом с hl2.exe), без пробелов. -> { args, files: { "cfg/x.cfg": текст } }.
const STOCK_HUD_CFG = "hlsr-render-hud.cfg";
function configArguments({ ownConfig, ownHud, dir }) {
  if (ownConfig !== false && ownHud !== false) return { args: [], files: {} };
  const args = ["-insert_search_path", dir];
  const files = {};
  if (ownConfig === false) {
    const empty = "// HLSR: стандартный конфиг для рендера, без настроек игрока.\n";
    files["cfg/config.cfg"] = empty;
    files["cfg/autoexec.cfg"] = empty;
    if (ownHud !== false) args.push("+exec", personalization.PERSONALIZATION_FILE);
  } else {
    files[`cfg/${STOCK_HUD_CFG}`] = personalization.toCfgLines(personalization.defaultPersonalization());
    args.push("+exec", STOCK_HUD_CFG);
  }
  return { args, files };
}

// config — аргументы configArguments (до +startdemos: cfg исполняются раньше демок).
function gameArguments({ width, height, fps, keyhud, integrity, audio, demos, movie, logFile, config }) {
  const args = ["-steam", "-game", "ghostingmod", "-windowed", "-noborder", "-w", String(width), "-h", String(height),
    "-novid", "-multirun",
    "+con_logfile", logFile, "+con_timestamp", "0",
    // developer 1: иначе startupmenu грузит фоновую карту и обрывает первую демку.
    "+developer", "1", "+con_drawnotify", "0",
    // Без фокуса окна (скрытый стол) движок иначе спит и глушит звук — и в WAV.
    "+snd_mute_losefocus", "0", "+engine_no_focus_sleep", "0",
    // Опережение звука в WAV = snd_mixahead + 1 кадр: фиксируем его.
    "+snd_mixahead", "0.1",
    // volume попадает в WAV; 1.0 бывает с клиппингом.
    "+volume", "0.8",
    "+host_framerate", String(fps),
    "+gh_keyshud", keyhud ? "1" : "0",
    // Проверка данных Ghosting («GH DEMO DATA: INTACT») — по выбору игрока, по умолчанию нет.
    "+gh_demo_overlay", integrity ? "1" : "0",
    // Служебные надписи из autoexec игрока — всегда без них.
    "+cl_showpos", "0", "+cl_showfps", "0", "+net_graph", "0",
    // Плавный вид: рывок камеры не считается телепортом, интерполяция не сбрасывается.
    "+demo_avellimit", "99999", "+demo_interplimit", "99999",
    // Игрок и проп — на одной шкале времени (как gh_playdemo в моде): иначе
    // проп в руках (+use, грави-пушка) отстаёт и отлетает на прыжках.
    "+demo_legacy_rollback", "0",
    ...(config || []),
    // stuffcmds даёт каждой +команде одно значение (следующий аргумент):
    // список демо и формат записи — одним аргументом в кавычках.
    "+startdemos", demos.join(" "),
    "+startmovie", `${movie} tga${audio ? " wav" : ""}`];
  return args;
}

// Строка для CreateProcessW по правилам CommandLineToArgvW.
function quoteArg(arg) {
  const text = String(arg);
  if (text && !/[\s"]/.test(text)) return text;
  let out = "\"";
  let slashes = 0;
  for (const ch of text) {
    if (ch === "\\") {
      slashes++;
      continue;
    }
    if (ch === "\"") {
      out += "\\".repeat(slashes * 2 + 1) + "\"";
    } else out += "\\".repeat(slashes) + ch;
    slashes = 0;
  }
  return out + "\\".repeat(slashes * 2) + "\"";
}

function commandLine(exe, args) {
  return [exe].concat(args).map(quoteArg).join(" ");
}

// Опережение микшера при +snd_mixahead 0.1 (замер спайка: 0,133 / 0,117 / 0,107 с).
const mixerLead = (fps) => 0.1 + 1 / fps;

module.exports = {
  configArguments,
  STOCK_HUD_CFG,
  MAX_BATCH,
  MAX_COMMAND,
  batchLimit,
  mixerLead,
  planSegments,
  batches,
  clockLabel,
  timelineOf,
  parseLogLine,
  demoKey,
  tgaInfo,
  wavLayout,
  fixWavFile,
  wavHeader,
  WavJoiner,
  gameArguments,
  quoteArg,
  commandLine,
};
