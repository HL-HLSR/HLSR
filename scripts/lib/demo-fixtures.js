"use strict";

// Синтетические демо и плейлисты для самотестов инструмента «Демо»:
// заголовки GoldSrc/Source и GHPL v1 того же формата, что пишут игры.
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

function goldsrcDemo(options) {
  const opts = Object.assign({
    map: "c1a0",
    gameDir: "valve_WON",
    protocol: 5,
    entries: [
      { type: 0, description: "LOADING", duration: 0.5, frames: 10 },
      { type: 1, description: "Playback", duration: 12.25, frames: 1225 },
    ],
    dataBytes: 2000,
  }, options);
  const header = Buffer.alloc(544);
  header.write("HLDEMO\0\0", 0, "latin1");
  header.writeInt32LE(opts.protocol, 8);
  header.writeInt32LE(opts.netProtocol || 48, 12);
  header.write(opts.map, 16, "latin1");
  header.write(opts.gameDir, 276, "latin1");
  header.writeInt32LE(1234, 536);
  header.writeInt32LE(opts.directoryOffset === undefined ? 544 + opts.dataBytes : opts.directoryOffset, 540);
  const dir = Buffer.alloc(4 + opts.entries.length * 92);
  dir.writeInt32LE(opts.entryCount === undefined ? opts.entries.length : opts.entryCount, 0);
  // Данные делятся между записями без пересечений, как в настоящей демке.
  const share = Math.floor(opts.dataBytes / Math.max(1, opts.entries.length));
  opts.entries.forEach((entry, index) => {
    const at = 4 + index * 92;
    dir.writeInt32LE(entry.type, at);
    dir.write(entry.description, at + 4, "latin1");
    dir.writeFloatLE(entry.duration, at + 76);
    dir.writeInt32LE(entry.frames, at + 80);
    dir.writeInt32LE(544 + index * share, at + 84);
    dir.writeInt32LE(share, at + 88);
  });
  return Buffer.concat([header, Buffer.alloc(opts.dataBytes), dir]);
}

// Демка GoldSrc с настоящими кадрами (разбор карт, кнопок и загрузок):
// loading и playback — [{ type, time, frame, buttons, viewmodel, message, length }],
// тип 0/1 — сетевой кадр (DemoInfo 436 + 28 + длина + сообщение), 3 — 64 байта,
// 4 — 32 байта; type 5 в конец каждой записи директории добавляется сам.
function goldsrcFramesDemo(options) {
  const opts = Object.assign({ map: "c1a0", gameDir: "valve_WON", netProtocol: 47, duration: 5, loading: [], playback: [] }, options);
  const frames = (list) => {
    const chunks = [];
    for (const f of list.concat([{ type: 5, time: list.length ? list[list.length - 1].time : 0 }])) {
      const head = Buffer.alloc(9);
      head[0] = f.type;
      head.writeFloatLE(f.time || 0, 1);
      head.writeInt32LE(f.frame || 0, 5);
      chunks.push(head);
      if (f.type === 0 || f.type === 1) {
        const info = Buffer.alloc(436 + 28 + 4);
        info.writeUInt16LE(f.buttons || 0, 236 + 30);
        info.writeInt32LE(f.viewmodel || 0, 432);
        const message = Buffer.from(f.message || "", "latin1");
        info.writeInt32LE(f.length === undefined ? message.length : f.length, 464);
        chunks.push(info, message);
      } else if (f.type === 3) chunks.push(Buffer.alloc(64));
      else if (f.type === 4) chunks.push(Buffer.alloc(32));
    }
    return Buffer.concat(chunks);
  };
  const loading = frames(opts.loading);
  const playback = frames(opts.playback);
  const header = Buffer.alloc(544);
  header.write("HLDEMO\0\0", 0, "latin1");
  header.writeInt32LE(5, 8);
  header.writeInt32LE(opts.netProtocol, 12);
  header.write(opts.map, 16, "latin1");
  header.write(opts.gameDir, 276, "latin1");
  header.writeInt32LE(544 + loading.length + playback.length, 540);
  const dir = Buffer.alloc(4 + 2 * 92);
  dir.writeInt32LE(2, 0);
  [["LOADING", 0, 0, 544, loading.length], ["Playback", 1, opts.duration, 544 + loading.length, playback.length]].forEach(([name, type, duration, offset, length], index) => {
    const at = 4 + index * 92;
    dir.writeInt32LE(type, at);
    dir.write(name, at + 4, "latin1");
    dir.writeFloatLE(duration, at + 76);
    dir.writeInt32LE(opts.playback.length, at + 80);
    dir.writeInt32LE(offset, at + 84);
    dir.writeInt32LE(length, at + 88);
  });
  return Buffer.concat([header, loading, playback, dir]);
}

function sourceDemo(options) {
  const opts = Object.assign({
    map: "d1_trainstation_01",
    gameDir: "ghostingmod",
    protocol: 3,
    playback: 3.21,
    ticks: 321,
    frames: 313,
    signon: 200,
    client: "runner",
    dataBytes: 500,
  }, options);
  // body — кадры демки (sourceFrames) вместо нулей.
  if (opts.body && !(options && options.signon !== undefined)) opts.signon = 0;
  const header = Buffer.alloc(1072);
  header.write("HL2DEMO\0", 0, "latin1");
  header.writeInt32LE(opts.protocol, 8);
  header.writeInt32LE(24, 12);
  header.write("localhost:0", 16, "latin1");
  header.write(opts.client, 276, "utf8");
  header.write(opts.map, 536, "latin1");
  header.write(opts.gameDir, 796, "latin1");
  header.writeFloatLE(opts.playback, 1056);
  header.writeInt32LE(opts.ticks, 1060);
  header.writeInt32LE(opts.frames, 1064);
  header.writeInt32LE(opts.signon, 1068);
  return Buffer.concat([header, opts.body || Buffer.alloc(opts.signon + opts.dataBytes)]);
}

// Кадры демки Source протокола 3: команды консоли [тик, строка] между пакетами,
// в конце dem_stop (как пишет движок, без полного тика).
function sourceFrames(commands, options) {
  const opts = Object.assign({ stop: true }, options);
  const parts = [];
  const head = (cmd, tick) => {
    const b = Buffer.alloc(5);
    b[0] = cmd;
    b.writeInt32LE(tick, 1);
    return b;
  };
  const int = (value) => {
    const b = Buffer.alloc(4);
    b.writeInt32LE(value);
    return b;
  };
  parts.push(head(3, 0));
  for (const [tick, text] of commands) {
    // Пакет (cmdinfo 76 байт, seqIn, seqOut, длина, данные) и usercmd — как в настоящих демках.
    parts.push(head(2, tick), Buffer.alloc(76 + 8), int(3), Buffer.from([1, 2, 3]));
    parts.push(head(5, tick), int(tick), int(2), Buffer.from([9, 9]));
    const data = Buffer.from(`${text}\0`, "latin1");
    parts.push(head(4, tick), int(data.length), data);
  }
  if (opts.stop) parts.push(Buffer.from([7, 0, 0]));
  return Buffer.concat(parts);
}

function playlistBuffer(entries, options) {
  const opts = Object.assign({ total: 10, incomplete: 0, version: 1 }, options);
  const u16 = (value) => { const b = Buffer.alloc(2); b.writeUInt16LE(value); return b; };
  const f32 = (value) => { const b = Buffer.alloc(4); b.writeFloatLE(value); return b; };
  const u64 = (value) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(value)); return b; };
  const parts = [Buffer.from("GHPL", "latin1"), Buffer.from([opts.version, opts.incomplete]), u16(entries.length), f32(opts.total)];
  for (const entry of entries) {
    const file = Buffer.from(entry.file, "utf8");
    const map = Buffer.from(entry.map, "utf8");
    parts.push(u16(file.length), file, Buffer.from([map.length]), map, f32(entry.start), f32(entry.end), u64(entry.size),
      Buffer.from(entry.sha256 || "ab".repeat(32), "latin1"));
  }
  return Buffer.concat(parts);
}

// Ран Ghosting в папке: <dir>/<run>/segmentNNN.dem + run.ghplaylist.
// options — поля плейлиста (incomplete и т. п.), см. playlistBuffer.
function writeGhostingRun(demosDir, run, segments, options) {
  const dir = path.join(demosDir, run);
  fs.mkdirSync(dir, { recursive: true });
  let timer = 0;
  const entries = segments.map((segment, index) => {
    const name = `segment${String(index + 1).padStart(3, "0")}.dem`;
    const data = sourceDemo({ map: segment.map, playback: segment.duration, ticks: Math.round(segment.duration * 100), frames: Math.max(1, Math.round(segment.duration * 100)),
      body: segment.commands ? sourceFrames(segment.commands) : undefined });
    fs.writeFileSync(path.join(dir, name), data);
    const entry = { file: `demos/${run}/${name}`, map: segment.map, start: timer, end: timer + segment.duration, size: data.length,
      sha256: require("crypto").createHash("sha256").update(data).digest("hex") };
    timer += segment.duration + 0.01;
    return entry;
  });
  const file = path.join(dir, "run.ghplaylist");
  fs.writeFileSync(file, playlistBuffer(entries, Object.assign({ total: timer }, options)));
  return file;
}

// GHRD v1 мода (gh2013 tools/demo-recording/GHRD_SPEC.md): тело записи из полей
// [["f32", 1], ["u8", 2], ["u16", 3], ["str8", "map"]].
function ghrdBody(fields) {
  return Buffer.concat(fields.map(([kind, value]) => {
    if (kind === "str8") {
      const text = Buffer.from(value, "latin1");
      return Buffer.concat([Buffer.from([text.length]), text]);
    }
    const b = Buffer.alloc({ u8: 1, u16: 2, f32: 4 }[kind]);
    if (kind === "u8") b[0] = value;
    else if (kind === "u16") b.writeUInt16LE(value);
    else b.writeFloatLE(value);
    return b;
  }));
}

// Команды «_ghrd <base64url>»: запись (type, bodyLen, тело, MAC 16 байт) в parts частях.
function ghrdCommands(seq, type, body, parts) {
  const count = parts || 1;
  const length = Buffer.alloc(2);
  length.writeUInt16LE(body.length);
  const record = Buffer.concat([Buffer.from([type]), length, body, Buffer.alloc(16)]);
  const size = Math.ceil(record.length / count);
  const out = [];
  for (let part = 0; part < count; part++) {
    const head = Buffer.alloc(7);
    head[0] = 1;
    head.writeUInt32LE(seq, 1);
    head[5] = part;
    head[6] = count;
    out.push(`_ghrd ${Buffer.concat([head, record.subarray(part * size, (part + 1) * size)]).toString("base64url")}`);
  }
  return out;
}

// FFmpeg для тестов: HLSR_DEMO_FFMPEG (папка bin или exe), иначе из PATH
// (только Linux: на Windows лаунчер берёт FFmpeg из папки инструментов).
function findFfmpeg() {
  if (!process.env.HLSR_DEMO_FFMPEG && process.platform === "win32") return null;
  const exe = process.platform === "win32" ? "ffmpeg.exe" : "ffmpeg";
  const probe = process.platform === "win32" ? "ffprobe.exe" : "ffprobe";
  const override = process.env.HLSR_DEMO_FFMPEG;
  let candidate = null;
  if (override) {
    try {
      candidate = fs.statSync(override).isDirectory()
        ? { ffmpeg: path.join(override, exe), ffprobe: path.join(override, probe), dir: override }
        : { ffmpeg: override, ffprobe: path.join(path.dirname(override), probe), dir: path.dirname(override) };
    } catch (error) {
      candidate = null;
    }
  } else candidate = { ffmpeg: "ffmpeg", ffprobe: "ffprobe", dir: null };
  if (!candidate) return null;
  const run = spawnSync(candidate.ffmpeg, ["-version"], { encoding: "utf8", windowsHide: true });
  return run.status === 0 ? candidate : null;
}

function ffprobeJson(ffprobe, file) {
  const run = spawnSync(ffprobe, ["-v", "error", "-show_entries", "stream=codec_type,codec_name,avg_frame_rate,width,height,bit_rate:format=duration,size", "-of", "json", file],
    { encoding: "utf8", windowsHide: true });
  if (run.status !== 0) throw new Error(run.stderr);
  return JSON.parse(run.stdout);
}

function memoryStore(initial) {
  const data = JSON.parse(JSON.stringify(initial || { manual: [], export: null, outputDir: null }));
  return { data, get: (key) => data[key], set: (key, value) => { data[key] = JSON.parse(JSON.stringify(value)); return true; } };
}

module.exports = { goldsrcDemo, goldsrcFramesDemo, sourceDemo, sourceFrames, ghrdBody, ghrdCommands, playlistBuffer, writeGhostingRun, findFfmpeg, ffprobeJson, memoryStore };
