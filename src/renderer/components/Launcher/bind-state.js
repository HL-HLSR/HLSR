const binds = require("../../../shared/game-binds");
const bindPresets = require("../../../shared/bind-presets");

// Итог для каждой клавиши: то, что будет в игре после запуска. fileBinds —
// бинды из cfg игры ({ KEY: { command, source } }), overrides —
// переназначения лаунчера ({ KEY: { command, base } }).
function effectiveBinds(fileBinds, overrides) {
  const result = {};
  for (const [key, entry] of Object.entries(fileBinds || {})) {
    result[key] = { command: entry.command, source: entry.source, overridden: false };
  }
  for (const [key, entry] of Object.entries(binds.sanitizeOverrides(overrides))) {
    // Возврат (command = base) — это снова бинд игры, а не лаунчера.
    if (entry.command === entry.base) {
      if (entry.command) result[key] = { command: entry.command, source: "config.cfg", overridden: false };
      else delete result[key];
    } else {
      result[key] = { command: entry.command, source: null, overridden: true, base: entry.base };
    }
  }
  for (const key of Object.keys(result)) if (!result[key].command) delete result[key];
  return result;
}

// Команда → клавиши, на которых она стоит.
function keysByCommand(effective) {
  const map = {};
  for (const [key, entry] of Object.entries(effective)) {
    const command = bindPresets.commandKey(entry.command);
    (map[command] = map[command] || []).push(key);
  }
  return map;
}

// Команда → клавиши, где она стоит целиком или частью составного бинда,
// в том числе внутри перебинда («save x; bind mouse4 +tau» находит +tau) и
// за цепочкой alias игрока («re4» → … → «bind f obbo1000»). Для вкладки
// «Скрипты»: скрипты из импортированного конфига часто так и назначены.
function keysByPart(effective, aliases) {
  const map = {};
  const aliasMap = binds.aliasMap(aliases);
  for (const [key, entry] of Object.entries(effective)) {
    const parts = new Set([bindPresets.commandKey(entry.command)]);
    for (const part of binds.expandCommand(entry.command, aliasMap)) parts.add(bindPresets.commandKey(part));
    for (const part of parts) (map[part] = map[part] || []).push(key);
  }
  return map;
}

// Что было на клавише до лаунчера: base переназначения или бинд из cfg.
function baseOf(key, fileBinds, overrides) {
  const entry = binds.sanitizeOverrides(overrides)[key];
  if (entry) return entry.base;
  return fileBinds && fileBinds[key] ? fileBinds[key].command : "";
}

export { effectiveBinds, keysByCommand, keysByPart, baseOf };
