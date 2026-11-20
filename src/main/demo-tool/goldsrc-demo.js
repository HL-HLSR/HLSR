"use strict";

const fs = require("fs");

// Разбор кадров демки GoldSrc (HLDEMO, протокол демо 5) для таймлайна и
// KeyHUD: смены карт (svc_serverinfo -> «maps/<карта>.bsp» в сетевом
// сообщении) и кнопки usercmd каждого сетевого кадра. Раскладка кадров — как
// в hldemo-rs (YaLTeR): u8 тип, f32 время, u32 номер; сетевой кадр — DemoInfo
// 436 байт (время 4 + ref_params 232 + usercmd 52 + movevars 132 + view 12 +
// viewmodel 4), 28 байт последовательностей, u32 длина и сообщение.
// Файл читается кусками (память не растёт с размером демки), битый кадр
// останавливает разбор: возвращается то, что успели прочитать, и broken.
// Файл недоверенный (скачанная демка): записи директории не пересекаются
// (каждый байт читается один раз), смен карт и нажатий — не больше пределов.

const HEADER = 544;
const DIR_ENTRY = 92;
const MAX_ENTRIES = 1024;
const DEMO_INFO = 436;
const USERCMD_AT = 4 + 232;
const BUTTONS_AT = USERCMD_AT + 30;
// Модель оружия в руках (0 — без оружия): после usercmd, movevars 132 и view 12.
const VIEWMODEL_AT = USERCMD_AT + 52 + 132 + 12;
const SEQUENCE_INFO = 28;
// Сетевое сообщение GoldSrc — до 64 КБ (NET_MAX_PAYLOAD), с запасом.
const MAX_MESSAGE = 1 << 20;
const CHUNK = 1 << 20;
const FIXED = { 2: 0, 3: 64, 4: 32, 6: 84, 7: 8 };
const MAP_RE = /maps\/([A-Za-z0-9_-]{1,64})\.bsp/;
// Пределы на демку: в ране HL1 смен карт — десятки, нажатий — десятки тысяч.
const MAX_MAP_CHANGES = 256;
const MAX_BUTTON_CHANGES = 500000;
const MAX_FILE = 2 * 1024 * 1024 * 1024;
const round3 = (value) => Math.round(value * 1000) / 1000;

// Биты IN_* usercmd, которые показывает KeyHUD.
const BUTTONS = Object.freeze({ attack: 1, jump: 2, duck: 4, forward: 8, back: 16, use: 32, moveleft: 512, moveright: 1024, attack2: 2048, reload: 8192 });

// Последовательное чтение отрезка файла окнами по CHUNK байт.
class Reader {
  constructor(fd, start, end) {
    this.fd = fd;
    this.pos = start;
    this.end = end;
    this.buffer = Buffer.alloc(0);
    this.at = 0;
  }

  async need(n) {
    if (this.buffer.length - this.at >= n) return true;
    const rest = this.buffer.subarray(this.at);
    const want = Math.min(Math.max(n - rest.length, CHUNK), this.end - this.pos);
    if (want <= 0 || rest.length + want < n) return false;
    const chunk = Buffer.alloc(want);
    const { bytesRead } = await this.fd.read(chunk, 0, want, this.pos);
    this.pos += bytesRead;
    this.buffer = Buffer.concat([rest, chunk.subarray(0, bytesRead)]);
    this.at = 0;
    return this.buffer.length >= n;
  }

  take(n) {
    const out = this.buffer.subarray(this.at, this.at + n);
    this.at += n;
    return out;
  }
}

// Директория в конце файла: записи между заголовком и директорией, без
// пересечений (подделка с тысячей записей на один участок не множит работу), иначе null.
async function directory(read, size) {
  const head = await read(0, HEADER);
  if (head.length < HEADER || head.toString("latin1", 0, 8) !== "HLDEMO\0\0") return null;
  const offset = head.readInt32LE(540);
  if (offset < HEADER || offset + 4 > size) return null;
  const count = (await read(offset, 4)).readInt32LE(0);
  if (count < 1 || count > MAX_ENTRIES || offset + 4 + count * DIR_ENTRY > size) return null;
  const table = await read(offset + 4, count * DIR_ENTRY);
  const entries = [];
  for (let i = 0; i < count; i++) {
    const at = i * DIR_ENTRY;
    const entry = { type: table.readInt32LE(at), duration: table.readFloatLE(at + 76), offset: table.readInt32LE(at + 84), length: table.readInt32LE(at + 88) };
    if (!(entry.duration >= 0 && entry.duration <= 1e6) || entry.offset < HEADER || entry.length < 0 || entry.offset + entry.length > offset) return null;
    entries.push(entry);
  }
  const sorted = entries.slice().sort((a, b) => a.offset - b.offset);
  for (let i = 1; i < sorted.length; i++) if (sorted[i - 1].offset + sorted[i - 1].length > sorted[i].offset) return null;
  return entries;
}

// -> { ok, broken, duration, mapChanges: [{ t, map, gap }], buttons: [{ t, buttons }], startGap }
// t — время кадра в сегменте «Playback» (как длительность в директории);
// buttons — только смены состояния кнопок. Кадры загрузки (LOADING) дают
// только карты (их время — 0). Для привязки видео bxt-rs ко времени демки
// (bxt_cap_skip_non_gameplay_frames пропускает кадры загрузки):
// gap смены карты — от svc_serverinfo до первого кадра ClientData (тип 4:
// клиент снова в игре); startGap — начало демки до 7-го кадра, если в руках
// нет оружия (правило bxt-rs), иначе 0.
async function scanDemo(file, options) {
  const signal = options && options.signal;
  const handle = await fs.promises.open(file, "r");
  try {
    const size = (await handle.stat()).size;
    if (size > MAX_FILE) return { ok: false, code: "DEMO_TOO_LARGE" };
    const read = async (position, length) => {
      const buffer = Buffer.alloc(length);
      const { bytesRead } = await handle.read(buffer, 0, length, position);
      return buffer.subarray(0, bytesRead);
    };
    const entries = size >= HEADER ? await directory(read, size) : null;
    if (!entries) return { ok: false, code: "DEMO_DIRECTORY_INVALID" };

    const out = { ok: true, broken: false, duration: 0, mapChanges: [], buttons: [], startGap: null };
    let previous = null;
    let waiting = null;
    let armed = null;
    for (const entry of entries) {
      const playback = entry.type !== 0;
      if (playback) out.duration += entry.duration;
      const reader = new Reader(handle, entry.offset, entry.offset + entry.length);
      const broken = () => {
        out.broken = true;
      };
      for (;;) {
        if (signal && signal.aborted) throw Object.assign(new Error("Отменено"), { code: "CANCELLED" });
        if (!(await reader.need(9))) break;
        const frame = reader.take(9);
        const type = frame[0];
        const time = frame.readFloatLE(1);
        const t = playback && Number.isFinite(time) ? round3(Math.max(0, time)) : 0;
        if (type === 5) break;
        if (playback && out.startGap === null && frame.readInt32LE(5) >= 7) out.startGap = armed === true ? 0 : t;
        if (playback && type === 4 && waiting) {
          waiting.gap = round3(Math.max(0, t - waiting.t));
          waiting = null;
        }
        let skip = FIXED[type];
        if (type === 0 || type === 1) {
          if (!(await reader.need(DEMO_INFO + SEQUENCE_INFO + 4))) {
            broken();
            break;
          }
          const info = reader.take(DEMO_INFO + SEQUENCE_INFO + 4);
          const length = info.readInt32LE(DEMO_INFO + SEQUENCE_INFO);
          if (length < 0 || length > MAX_MESSAGE || !(await reader.need(length))) {
            broken();
            break;
          }
          // Карта — первая «maps/<карта>.bsp» сообщения (svc_serverinfo и список ресурсов).
          const match = MAP_RE.exec(reader.take(length).toString("latin1"));
          const last = out.mapChanges[out.mapChanges.length - 1];
          if (match && !(last && last.map === match[1])) {
            if (out.mapChanges.length >= MAX_MAP_CHANGES) out.broken = true;
            else {
              const change = { t, map: match[1], gap: 0 };
              out.mapChanges.push(change);
              if (playback) waiting = change;
            }
          }
          if (playback && type === 1) {
            if (armed === null) armed = info.readInt32LE(VIEWMODEL_AT) !== 0;
            const buttons = info.readUInt16LE(BUTTONS_AT);
            if (buttons !== previous && out.buttons.length >= MAX_BUTTON_CHANGES) out.broken = true;
            else if (buttons !== previous) {
              out.buttons.push({ t, buttons });
              previous = buttons;
            }
          }
          continue;
        }
        if (type === 8) {
          if (!(await reader.need(8))) {
            broken();
            break;
          }
          const sample = reader.take(8).readInt32LE(4);
          skip = sample >= 0 && sample <= MAX_MESSAGE ? sample + 16 : -1;
        } else if (type === 9) {
          if (!(await reader.need(4))) {
            broken();
            break;
          }
          const length = reader.take(4).readInt32LE(0);
          skip = length >= 0 && length <= MAX_MESSAGE ? length : -1;
        }
        if (skip === undefined || skip < 0 || !(await reader.need(skip))) {
          broken();
          break;
        }
        reader.take(skip);
      }
    }
    out.duration = round3(out.duration);
    if (out.startGap === null) out.startGap = 0;
    return out;
  } finally {
    await handle.close();
  }
}
module.exports = { BUTTONS, MAX_MAP_CHANGES, MAX_BUTTON_CHANGES, scanDemo };
