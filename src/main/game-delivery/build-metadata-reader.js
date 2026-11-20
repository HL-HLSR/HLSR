"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const buildMetadata = require("../../shared/build-metadata");
const { assertInside } = require("./paths");

const RELATIVE_PATH = buildMetadata.RELATIVE_PATH;
const MAX_BYTES = buildMetadata.MAX_BYTES;

function assertRealpathContained(installRoot, target) {
  let realRoot;
  try {
    realRoot = fs.realpathSync(installRoot);
  } catch (e) {
    return;
  }

  let realTarget;
  try {
    realTarget = fs.realpathSync(target);
  } catch (e) {
    return;
  }

  const prefix = realRoot.endsWith(path.sep) ? realRoot : realRoot + path.sep;
  if (
    realTarget.toLowerCase() !== realRoot.toLowerCase() &&
    !realTarget.toLowerCase().startsWith(prefix.toLowerCase())
  )
    throw new buildMetadata.BuildMetadataError(
      "BUILD_METADATA_INVALID",
      "Файл метаданных ведёт за пределы установки (symlink/junction)",
      { target, realTarget }
    );
}

function read(installRoot, options) {
  const opts = options || {};
  const target = assertInside(
    installRoot,
    path.join(installRoot, RELATIVE_PATH.replace(/\//g, path.sep)),
    "Файл метаданных сборки"
  );

  let stat;
  try {
    stat = fs.lstatSync(target);
  } catch (e) {
    return { present: false, relativePath: RELATIVE_PATH };
  }

  if (stat.isSymbolicLink())
    throw new buildMetadata.BuildMetadataError(
      "BUILD_METADATA_INVALID",
      "Файл метаданных сборки является symlink",
      { target }
    );

  if (!stat.isFile())
    throw new buildMetadata.BuildMetadataError(
      "BUILD_METADATA_INVALID",
      "По пути метаданных сборки лежит не файл",
      { target }
    );

  if (stat.size > MAX_BYTES)
    throw new buildMetadata.BuildMetadataError(
      "BUILD_METADATA_INVALID",
      `Файл метаданных слишком большой: ${stat.size} байт при пределе ${MAX_BYTES}`,
      { target, size: stat.size, max: MAX_BYTES }
    );

  assertRealpathContained(installRoot, target);

  const buffer = fs.readFileSync(target);
  const sha256 = crypto.createHash("sha256").update(buffer).digest("hex");
  const raw = buffer.toString("utf8");

  const dto = buildMetadata.parse(raw, {
    expected: opts.expected,
    now: opts.now,
    source: RELATIVE_PATH,
  });

  return {
    present: true,
    dto,
    raw,
    sizeBytes: stat.size,
    sha256,
    relativePath: RELATIVE_PATH,
  };
}

module.exports = { read, RELATIVE_PATH, MAX_BYTES };
