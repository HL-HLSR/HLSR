"use strict";

const fs = require("fs");
const path = require("path");
const { spawn } = require("child_process");

const { DemoError } = require("./errors");
const { CPU_ENCODER, GPU_ENCODERS } = require("./planner");

// FFmpeg только кодирует и собирает MP4. Windows: закреплённая сборка
// gyan.dev, скачанная как инструмент лаунчера (config/tools/ffmpeg.manifest.json).
// Linux, первая версия: ffmpeg из PATH, без скачивания.
const PRODUCTION_BUILD = process.env.NODE_ENV === "production";
// Подмена пути к FFmpeg (папка bin или сам ffmpeg) — только dev и тесты.
const ENV_OVERRIDE = "HLSR_DEMO_FFMPEG";
const PINNED_VERSION = "9.0.2";
const PROBE_TIMEOUT_MS = 30000;
const VERSION_TIMEOUT_MS = 15000;
const PROBE_PIX_FMT = { h264_qsv: "nv12", h264_amf: "nv12" };

const exeName = (base, platform) => (platform === "win32" ? `${base}.exe` : base);

function locate({ platform, env, production, toolDir }) {
  const isProduction = production === undefined ? PRODUCTION_BUILD : production;
  const override = !isProduction && env ? env[ENV_OVERRIDE] : null;
  if (override) {
    try {
      const dir = fs.statSync(override).isDirectory() ? override : path.dirname(override);
      const ffmpeg = dir === override ? path.join(dir, exeName("ffmpeg", platform)) : override;
      return { ffmpeg, ffprobe: path.join(dir, exeName("ffprobe", platform)), source: "env" };
    } catch (error) {
      return null;
    }
  }
  if (platform === "win32") {
    const ffmpeg = toolDir ? path.join(toolDir, "ffmpeg.exe") : null;
    return ffmpeg && fs.existsSync(ffmpeg) ? { ffmpeg, ffprobe: path.join(toolDir, "ffprobe.exe"), source: "tool" } : null;
  }
  return { ffmpeg: "ffmpeg", ffprobe: "ffprobe", source: "path" };
}

// Процесс FFmpeg: прогресс из -progress pipe:1 (секунды выхода), отмена через
// AbortSignal гасит именно этот процесс по его PID.
function run(exe, args, options) {
  const opts = options || {};
  const spawnFn = opts.spawnFn || spawn;
  return new Promise((resolve, reject) => {
    if (opts.signal && opts.signal.aborted) return reject(new DemoError("CANCELLED"));
    let child;
    try {
      child = spawnFn(exe, args, { windowsHide: true, shell: false, stdio: ["ignore", "pipe", "pipe"], cwd: opts.cwd });
    } catch (error) {
      return reject(new DemoError("FFMPEG_START_FAILED", error.message));
    }
    let settled = false;
    let stdout = "";
    let stderr = "";
    let pending = "";
    let aborted = false;
    let timedOut = false;
    const kill = () => {
      try {
        child.kill("SIGKILL");
      } catch (error) {
        /* уже завершился */
      }
    };
    const onAbort = () => {
      aborted = true;
      kill();
    };
    const timer = opts.timeoutMs ? setTimeout(() => { timedOut = true; kill(); }, opts.timeoutMs) : null;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      if (opts.signal) opts.signal.removeEventListener("abort", onAbort);
      if (error) reject(error);
      else resolve(value);
    };
    if (opts.signal) opts.signal.addEventListener("abort", onAbort, { once: true });
    if (opts.onSpawn) opts.onSpawn(child);

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      if (opts.collect && stdout.length < 1024 * 1024) stdout += chunk;
      if (!opts.onProgress) return;
      pending += chunk;
      let newline;
      while ((newline = pending.indexOf("\n")) >= 0) {
        const match = /^out_time_us=(\d+)/.exec(pending.slice(0, newline).trim());
        pending = pending.slice(newline + 1);
        if (match) opts.onProgress(Number(match[1]) / 1e6);
      }
    });
    child.stderr.on("data", (chunk) => {
      stderr = (stderr + chunk).slice(-4000);
    });
    child.on("error", (error) =>
      finish(new DemoError(error.code === "ENOENT" ? "FFMPEG_MISSING" : "FFMPEG_START_FAILED", error.message)));
    child.on("close", (code) => {
      if (aborted) return finish(new DemoError("CANCELLED"));
      if (timedOut) return finish(new DemoError("FFMPEG_TIMEOUT", stderr.trim().slice(-1000)));
      if (code !== 0) return finish(new DemoError("FFMPEG_FAILED", stderr.trim().slice(-1000), { exitCode: code }));
      finish(null, { stdout, stderr });
    });
  });
}

async function version(exe, options) {
  const result = await run(exe, ["-version"], Object.assign({ collect: true, timeoutMs: VERSION_TIMEOUT_MS }, options));
  const firstLine = result.stdout.split(/\r?\n/)[0].trim();
  const match = /^ffmpeg version (\S+)/.exec(firstLine);
  return {
    firstLine,
    version: match ? match[1] : null,
    pinned: firstLine.startsWith(`ffmpeg version ${PINNED_VERSION}`),
    license: /--enable-gpl\b/.test(result.stdout) ? "GPL" : "LGPL",
  };
}

async function listEncoders(exe, options) {
  const result = await run(exe, ["-hide_banner", "-encoders"], Object.assign({ collect: true, timeoutMs: VERSION_TIMEOUT_MS }, options));
  const names = new Set();
  for (const line of result.stdout.split(/\r?\n/)) {
    const match = /^\s*[VAS][A-Z.]{5}\s+(\S+)/.exec(line);
    if (match) names.add(match[1]);
  }
  return names;
}

function probeArgs(encoder, width, height, fps) {
  return ["-hide_banner", "-nostdin", "-loglevel", "error", "-f", "lavfi", "-i", `testsrc2=size=${width}x${height}:rate=${fps}`,
    "-t", "1", "-c:v", encoder, "-pix_fmt", PROBE_PIX_FMT[encoder] || "yuv420p", "-f", "null", "-"];
}

// Пробы кодировщиков на сессию: GPU-кодировщик считается рабочим, только
// если короткое пробное кодирование нужного размера прошло.
class EncoderProbe {
  constructor(options) {
    const opts = options || {};
    this.exe = opts.exe;
    this.runFn = opts.runFn || run;
    this.listFn = opts.listFn || listEncoders;
    this.logger = opts.logger || { info() {}, warn() {} };
    this.signal = opts.signal;
    this.cache = new Map();
    this.listed = null;
  }

  names() {
    if (!this.listed) this.listed = Promise.resolve(this.listFn(this.exe, { signal: this.signal })).catch(() => new Set());
    return this.listed;
  }

  works(encoder, width, height, fps) {
    const key = `${encoder}@${width}x${height}@${fps}`;
    if (!this.cache.has(key))
      this.cache.set(key, (async () => {
        const names = await this.names();
        if (!names.has(encoder)) return false;
        if (encoder === CPU_ENCODER) return true;
        try {
          await this.runFn(this.exe, probeArgs(encoder, width, height, fps), { timeoutMs: PROBE_TIMEOUT_MS, signal: this.signal });
          this.logger.info(`[demo] проба ${key}: работает`);
          return true;
        } catch (error) {
          this.logger.info(`[demo] проба ${key}: не работает (${error.code})`);
          return false;
        }
      })());
    return this.cache.get(key);
  }

  // Базовая проба (1080p60) всех кодировщиков: список для окна экспорта.
  async summary() {
    const encoders = [];
    for (const id of GPU_ENCODERS.concat([CPU_ENCODER]))
      encoders.push({ id, kind: id === CPU_ENCODER ? "cpu" : "gpu", available: await this.works(id, 1920, 1080, 60) });
    const gpu = encoders.find((item) => item.kind === "gpu" && item.available);
    const cpu = encoders.find((item) => item.kind === "cpu" && item.available);
    return { encoders, gpu: gpu ? gpu.id : null, cpu: cpu ? cpu.id : null, auto: gpu ? gpu.id : cpu ? cpu.id : null };
  }
}

// Длительность каждого потока: format.duration — максимум по потокам, и
// дотянутый apad звук скрыл бы обрезанное видео.
async function ffprobe(exe, file, options) {
  const result = await run(exe, ["-v", "error", "-show_entries",
    "stream=codec_type,codec_name,avg_frame_rate,width,height,duration,nb_frames:format=duration", "-of", "json", file],
  Object.assign({ collect: true, timeoutMs: VERSION_TIMEOUT_MS }, options));
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw new DemoError("VERIFY_FAILED", "ffprobe вернул не JSON");
  }
}

module.exports = { ENV_OVERRIDE, PINNED_VERSION, PRODUCTION_BUILD, locate, run, version, listEncoders, probeArgs, EncoderProbe, ffprobe };
