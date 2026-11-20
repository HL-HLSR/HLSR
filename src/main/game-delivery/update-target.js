"use strict";

function fromLocal(manifest) {
  if (!manifest || !manifest.version) return null;

  return {
    source: "local",
    version: manifest.version,
    updateRequired: false,
    minimumLauncherVersion: manifest.minimumLauncherVersion || null,
    publishedAt: manifest.publishedAt || null,
    expectedInstalledSize: manifest.expectedInstalledSize || null,
    patches: manifest.patches || [],
    fullArchive: manifest.baseArchive || null,
    repairMirror: null,
    filesManifest: manifest.filesManifest || null,
    buildMetadata: manifest.buildMetadata || null,
    notes: typeof manifest.notes === "string" ? manifest.notes : null,
  };
}

function fromChannel(manifest) {
  if (!manifest || !manifest.version) return null;

  return {
    source: "channel",
    version: manifest.version,
    updateRequired: !!manifest.updateRequired,
    minimumLauncherVersion: manifest.minimumLauncherVersion || null,
    publishedAt: manifest.publishedAt || null,
    expectedInstalledSize: manifest.expectedInstalledSize || null,
    patches: manifest.patches || [],
    fullArchive: manifest.fullArchive || null,
    repairMirror: manifest.repairMirror || null,
    filesManifest: manifest.filesManifest || null,
    buildMetadata: manifest.buildMetadata || null,
    notes: typeof manifest.notes === "string" ? manifest.notes : null,
  };
}

module.exports = { fromLocal, fromChannel };
