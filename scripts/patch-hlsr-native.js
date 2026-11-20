#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const EXPECTED_VERSION = "1.0.8";
const EXPECTED_CALLBACKS = 10;
const LEGACY_SIGNATURE = "(Napi::CallbackInfo &info)";
const FIXED_SIGNATURE = "(const Napi::CallbackInfo &info)";

function occurrences(source, needle) {
  return source.split(needle).length - 1;
}

function fail(message) {
  process.stderr.write(`hlsr-native compatibility patch failed: ${message}\n`);
  process.exit(1);
}

let packagePath;
try {
  packagePath = require.resolve("hlsr-native/package.json", { paths: [ROOT] });
} catch (error) {
  fail("hlsr-native is not installed; run npm ci first");
}

const packageRoot = path.dirname(packagePath);
const packageJson = JSON.parse(fs.readFileSync(packagePath, "utf8"));
const sourcePath = path.join(packageRoot, "src", "main.cpp");

if (packageJson.version !== EXPECTED_VERSION) {
  fail(`expected hlsr-native ${EXPECTED_VERSION}, found ${packageJson.version}`);
}
if (!fs.existsSync(sourcePath)) {
  fail(`source file is missing: ${sourcePath}`);
}

const source = fs.readFileSync(sourcePath, "utf8");
const legacyCount = occurrences(source, LEGACY_SIGNATURE);
const fixedCount = occurrences(source, FIXED_SIGNATURE);

if (legacyCount === 0 && fixedCount === EXPECTED_CALLBACKS) {
  process.stdout.write("hlsr-native callback signatures are already compatible\n");
  process.exit(0);
}

if (legacyCount !== EXPECTED_CALLBACKS || fixedCount !== 0) {
  fail(
    `unexpected callback layout (legacy=${legacyCount}, fixed=${fixedCount}); ` +
      "review the dependency before changing this patch"
  );
}

const patched = source.split(LEGACY_SIGNATURE).join(FIXED_SIGNATURE);
fs.writeFileSync(sourcePath, patched, "utf8");

if (
  occurrences(patched, LEGACY_SIGNATURE) !== 0 ||
  occurrences(patched, FIXED_SIGNATURE) !== EXPECTED_CALLBACKS
) {
  fail("post-write verification failed");
}

process.stdout.write(
  `Patched ${EXPECTED_CALLBACKS} hlsr-native callback signatures for modern MSVC\n`
);
