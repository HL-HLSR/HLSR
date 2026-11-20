"use strict";

const MAX_FRAMES_DECODED = 512;
const MAX_PIXELS = 1024 * 1024;

function lzwDecode(data, minCodeSize, pixelCount) {
  const MAX_CODE = 4096;
  const prefix = new Int32Array(MAX_CODE);
  const suffix = new Uint8Array(MAX_CODE);
  const stack = new Uint8Array(MAX_CODE);

  const clearCode = 1 << minCodeSize;
  const endCode = clearCode + 1;

  let codeSize = minCodeSize + 1;
  let codeMask = (1 << codeSize) - 1;
  let next = endCode + 1;

  for (let i = 0; i < clearCode; i++) {
    prefix[i] = 0;
    suffix[i] = i;
  }

  const out = new Uint8Array(pixelCount);
  let outPos = 0;

  let bitBuffer = 0;
  let bitCount = 0;
  let dataPos = 0;
  let previous = -1;

  while (outPos < pixelCount) {
    while (bitCount < codeSize) {
      if (dataPos >= data.length) {
        return outPos > 0 ? out : null;
      }
      bitBuffer |= data[dataPos++] << bitCount;
      bitCount += 8;
    }

    const code = bitBuffer & codeMask;
    bitBuffer >>= codeSize;
    bitCount -= codeSize;

    if (code === clearCode) {
      codeSize = minCodeSize + 1;
      codeMask = (1 << codeSize) - 1;
      next = endCode + 1;
      previous = -1;
      continue;
    }
    if (code === endCode) break;

    let current = code;
    let stackPos = 0;

    if (code >= next) {
      if (previous < 0) return outPos > 0 ? out : null;
      stack[stackPos++] = suffix[previous];
      current = previous;
    }

    let guard = 0;
    while (current >= clearCode) {
      if (++guard > MAX_CODE) return outPos > 0 ? out : null;
      stack[stackPos++] = suffix[current];
      current = prefix[current];
    }
    stack[stackPos++] = suffix[current];

    while (stackPos > 0 && outPos < pixelCount) out[outPos++] = stack[--stackPos];

    if (previous >= 0 && next < MAX_CODE) {
      prefix[next] = previous;
      suffix[next] = current;
      next++;
      if ((next & codeMask) === 0 && next < MAX_CODE) {
        codeSize++;
        codeMask = (1 << codeSize) - 1;
      }
    }
    previous = code;
  }

  return out;
}

function readSubBlocks(buf, pos) {
  const parts = [];
  let total = 0;
  while (pos < buf.length) {
    const n = buf[pos];
    if (n === 0) { pos++; break; }
    pos++;
    if (pos + n > buf.length) return { data: null, pos: buf.length };
    parts.push(buf.subarray(pos, pos + n));
    total += n;
    pos += n;
  }
  return { data: parts.length ? Buffer.concat(parts, total) : Buffer.alloc(0), pos };
}

function skipSubBlocks(buf, pos) {
  while (pos < buf.length && buf[pos] !== 0) pos += buf[pos] + 1;
  return pos + 1;
}

function readPalette(buf, pos, size) {
  const pal = new Uint8Array(size * 3);
  for (let i = 0; i < size * 3 && pos + i < buf.length; i++) pal[i] = buf[pos + i];
  return pal;
}

function decodeGif(buf) {
  if (!Buffer.isBuffer(buf) || buf.length < 13) return null;
  if (buf.toString("ascii", 0, 3) !== "GIF") return null;

  const wide = buf.readUInt16LE(6);
  const tall = buf.readUInt16LE(8);
  if (wide <= 0 || tall <= 0 || wide * tall > MAX_PIXELS) return null;

  const packed = buf[10];
  let pos = 13;
  let globalPalette = null;
  if (packed & 0x80) {
    const size = 2 << (packed & 7);
    globalPalette = readPalette(buf, pos, size);
    pos += size * 3;
  }

  const canvas = Buffer.alloc(wide * tall * 4);
  const frames = [];

  let delayMs = 0;
  let transparentIndex = -1;
  let disposal = 0;

  while (pos < buf.length && frames.length < MAX_FRAMES_DECODED) {
    const block = buf[pos];

    if (block === 0x3b) break;

    if (block === 0x21) {
      const label = buf[pos + 1];
      pos += 2;
      if (label === 0xf9 && pos + 5 <= buf.length) {
        const flags = buf[pos + 1];
        disposal = (flags >> 2) & 7;
        transparentIndex = flags & 1 ? buf[pos + 4] : -1;
        delayMs = buf.readUInt16LE(pos + 2) * 10;
      }
      pos = skipSubBlocks(buf, pos);
      continue;
    }

    if (block !== 0x2c) { pos++; continue; }

    const fx = buf.readUInt16LE(pos + 1);
    const fy = buf.readUInt16LE(pos + 3);
    const fw = buf.readUInt16LE(pos + 5);
    const fh = buf.readUInt16LE(pos + 7);
    const lflags = buf[pos + 9];
    pos += 10;

    let palette = globalPalette;
    if (lflags & 0x80) {
      const size = 2 << (lflags & 7);
      palette = readPalette(buf, pos, size);
      pos += size * 3;
    }
    const interlaced = (lflags & 0x40) !== 0;

    const minCodeSize = buf[pos++];
    const sub = readSubBlocks(buf, pos);
    pos = sub.pos;

    if (!palette || !sub.data || fw <= 0 || fh <= 0) continue;

    const indices = lzwDecode(sub.data, minCodeSize, fw * fh);
    if (!indices) continue;

    for (let row = 0; row < fh; row++) {
      const srcRow = interlaced ? deinterlaceRow(row, fh) : row;
      const y = fy + srcRow;
      if (y < 0 || y >= tall) continue;

      for (let col = 0; col < fw; col++) {
        const x = fx + col;
        if (x < 0 || x >= wide) continue;

        const index = indices[row * fw + col];
        if (index === transparentIndex) continue;

        const p = (y * wide + x) * 4;
        const q = index * 3;
        canvas[p] = palette[q];
        canvas[p + 1] = palette[q + 1];
        canvas[p + 2] = palette[q + 2];
        canvas[p + 3] = 255;
      }
    }

    frames.push({ rgba: Buffer.from(canvas), delayMs: delayMs > 0 ? delayMs : 100 });

    if (disposal === 2) {
      for (let row = 0; row < fh; row++) {
        const y = fy + row;
        if (y < 0 || y >= tall) continue;
        const from = (y * wide + Math.max(0, fx)) * 4;
        const count = Math.min(fw, wide - fx) * 4;
        if (count > 0) canvas.fill(0, from, from + count);
      }
    }

    delayMs = 0;
    transparentIndex = -1;
    disposal = 0;
  }

  if (!frames.length) return null;
  return { wide, tall, frames };
}

function deinterlaceRow(row, height) {
  const p1 = Math.ceil(height / 8);
  const p2 = p1 + Math.ceil((height - 4) / 8);
  const p3 = p2 + Math.ceil((height - 2) / 4);
  if (row < p1) return row * 8;
  if (row < p2) return (row - p1) * 8 + 4;
  if (row < p3) return (row - p2) * 4 + 2;
  return (row - p3) * 2 + 1;
}

const MIN_INTERVAL_MS = 40;
const MAX_INTERVAL_MS = 120;

function clampInterval(ms) {
  if (!(ms > 0)) return 100;
  return Math.min(MAX_INTERVAL_MS, Math.max(MIN_INTERVAL_MS, Math.round(ms)));
}

function sampleFrames(decoded, maxFrames) {
  const src = decoded.frames;
  if (src.length <= maxFrames) {
    const total = src.reduce((sum, f) => sum + f.delayMs, 0);
    return {
      frames: src.map((f) => f.rgba),
      intervalMs: clampInterval(total / src.length),
    };
  }

  const totalMs = src.reduce((sum, f) => sum + f.delayMs, 0);
  const step = totalMs / maxFrames;

  const picked = [];
  let cursor = 0;
  let elapsed = 0;
  let want = step / 2;

  for (let i = 0; i < src.length && picked.length < maxFrames; i++) {
    elapsed += src[i].delayMs;
    while (picked.length < maxFrames && want <= elapsed) {
      picked.push(src[i].rgba);
      want += step;
    }
    cursor = i;
  }
  while (picked.length < maxFrames) picked.push(src[cursor].rgba);

  return { frames: picked, intervalMs: clampInterval(step) };
}

module.exports = { decodeGif, sampleFrames };
