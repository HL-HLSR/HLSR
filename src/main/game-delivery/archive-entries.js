"use strict";

// The bundled Linux 7za reports directory attributes as "Dn" (lowercase n).
// Treat attribute flags case-insensitively while retaining symlink detection.
const REPARSE_ATTRIBUTE = /(^|\s)[A-Z_]*L[A-Z_]*(\s|$)/i;
const DIRECTORY_ATTRIBUTE = /(^|\s)[A-Z_]*D[A-Z_]*(\s|$)/i;
const UNIX_SYMLINK_MODE = /^l[rwxsSt-]{9}/;

function parseListing(stdout) {
  const entries = [];
  const archive = {};
  const lines = String(stdout).split(/\r?\n/);

  let separatorSeen = false;
  let current = null;

  const flush = () => {
    if (current && Object.keys(current).length) {
      if (separatorSeen) entries.push(normalizeEntry(current));
      else Object.assign(archive, current);
    }
    current = null;
  };

  for (const line of lines) {
    if (/^-{10,}$/.test(line)) {
      flush();
      separatorSeen = true;
      continue;
    }

    if (line.trim() === "") {
      flush();
      continue;
    }

    const match = line.match(/^([A-Za-z][A-Za-z0-9 ]*?) = ?(.*)$/);
    if (!match) continue;

    if (!current) current = {};
    current[match[1].trim()] = match[2];
  }

  flush();

  return { archive, entries };
}

function normalizeEntry(raw) {
  const attributes = raw.Attributes || "";
  const size = Number(raw.Size || 0);

  return {
    path: raw.Path || "",
    size: Number.isFinite(size) ? size : 0,
    packedSize: Number(raw["Packed Size"] || 0) || 0,
    attributes,
    modified: raw.Modified || null,
    crc: raw.CRC || null,
    encrypted: raw.Encrypted === "+",
    isDirectory: DIRECTORY_ATTRIBUTE.test(attributes),
    isSymlink:
      REPARSE_ATTRIBUTE.test(attributes) || UNIX_SYMLINK_MODE.test(attributes),
  };
}

function classifyPath(entryPath) {
  const problems = [];
  const value = String(entryPath == null ? "" : entryPath);
  const segments = value.split(/[\\/]/);

  if (value.trim() === "") problems.push("empty-path");
  if (/^[A-Za-z]:/.test(value)) problems.push("drive-path");
  if (/^[\\/]{2}/.test(value)) problems.push("unc-path");
  else if (/^[\\/]/.test(value)) problems.push("absolute-path");
  if (segments.some((part) => part === "..")) problems.push("parent-traversal");
  if (value.indexOf("\0") !== -1) problems.push("null-byte");
  if (/[<>|"*?]/.test(value)) problems.push("invalid-character");

  const name = segments[segments.length - 1] || "";
  if (/^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(\.|$)/i.test(name))
    problems.push("reserved-device-name");

  return problems;
}

function inspectEntries(entries, options) {
  const opts = options || {};
  const maxEntries = opts.maxEntries || Infinity;
  const maxUncompressedSize = opts.maxUncompressedSize || Infinity;

  const unsafe = [];
  const symlinks = [];
  const encrypted = [];
  const roots = new Set();

  let files = 0;
  let directories = 0;
  let uncompressedSize = 0;

  for (const entry of entries) {
    const problems = classifyPath(entry.path);
    if (problems.length) unsafe.push({ path: entry.path, problems });
    if (entry.isSymlink) symlinks.push(entry.path);
    if (entry.encrypted) encrypted.push(entry.path);

    if (entry.isDirectory) directories++;
    else {
      files++;
      uncompressedSize += entry.size;
    }

    if (!problems.length) {
      const first = entry.path.split(/[\\/]/)[0];
      if (first) roots.add(first);
    }
  }

  const errors = [];
  if (unsafe.length) errors.push("unsafe-entry");
  if (symlinks.length) errors.push("symlink-entry");
  if (encrypted.length) errors.push("encrypted-entry");
  if (entries.length > maxEntries) errors.push("too-many-entries");
  if (uncompressedSize > maxUncompressedSize) errors.push("uncompressed-size-limit");

  return {
    ok: errors.length === 0,
    errors,
    unsafe,
    symlinks,
    encrypted,
    roots: Array.from(roots),
    counts: { total: entries.length, files, directories },
    uncompressedSize,
  };
}

module.exports = {
  parseListing,
  classifyPath,
  inspectEntries,
};
