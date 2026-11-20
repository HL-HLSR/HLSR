"use strict";

const semver = require("./semver");

function planUpdate(options) {
  const opts = options || {};
  const patches = opts.patches || [];
  const from = opts.fromVersion;
  const to = opts.toVersion;
  const fullSize = opts.fullSize || 0;

  if (!from) return { kind: "full", reason: "not-installed", steps: [], totalSize: fullSize };

  if (from === to) return { kind: "up-to-date", steps: [], totalSize: 0 };

  if (semver.compare(from, to) > 0)
    return { kind: "full", reason: "downgrade", steps: [], totalSize: fullSize };

  const byFrom = new Map();
  for (const patch of patches) {
    if (!byFrom.has(patch.fromVersion)) byFrom.set(patch.fromVersion, []);
    byFrom.get(patch.fromVersion).push(patch);
  }

  const queue = [{ version: from, steps: [] }];
  const visited = new Set([from]);
  let found = null;

  while (queue.length && !found) {
    const current = queue.shift();

    for (const patch of byFrom.get(current.version) || []) {
      if (visited.has(patch.toVersion)) continue;

      const steps = current.steps.concat([patch]);

      if (patch.toVersion === to) {
        found = steps;
        break;
      }

      visited.add(patch.toVersion);
      queue.push({ version: patch.toVersion, steps });
    }
  }

  if (!found)
    return {
      kind: "full",
      reason: "no-chain",
      steps: [],
      totalSize: fullSize,
      message: `Нет цепочки патчей от ${from} к ${to}`,
    };

  const totalSize = found.reduce((sum, patch) => sum + (patch.archive ? patch.archive.size : 0), 0);

  if (fullSize && totalSize >= fullSize)
    return {
      kind: "full",
      reason: "patches-larger",
      steps: found,
      totalSize,
      fullSize,
      message: `Патчи весят ${totalSize} байт против ${fullSize} у полной сборки`,
    };

  return { kind: "patches", steps: found, totalSize, fullSize };
}

module.exports = { planUpdate };
