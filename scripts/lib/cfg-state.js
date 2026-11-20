"use strict";

// Эмулятор выполнения cfg для тестов перехода (§11 замысла): что остаётся в игре
// после цепочки cfg — переменные, бинды и alias. Строки разбираются как в
// движке (gameConfig.lineCommands); exec верхнего уровня выполняется на месте,
// exec внутри alias — нет (это значение alias).

const { lineCommands } = require("../../src/shared/game-config");

const MAX_DEPTH = 8;

// readFile(name) — текст cfg или null; name — имя из exec в нижнем регистре,
// с прямыми слэшами. roots — с чего движок начинает, по порядку.
function runChain(readFile, roots) {
  const cvars = new Map();
  const binds = new Map();
  const aliases = new Map();
  const visited = new Set();

  const run = (raw, depth) => {
    const name = String(raw).replace(/\\/g, "/").toLowerCase();
    if (depth > MAX_DEPTH || visited.has(name)) return;
    const text = readFile(name);
    if (text === null || text === undefined) return;
    visited.add(name);
    for (const line of String(text).split("\n")) {
      for (const command of lineCommands(line.replace(/\r$/, ""))) {
        const args = command.args.map((arg) => arg.value);
        const verb = args[0].toLowerCase();
        if (verb === "exec") {
          if (args.length >= 2) run(args[1], depth + 1);
        } else if (verb === "unbindall") {
          binds.clear();
        } else if (verb === "bind") {
          if (args.length >= 3) binds.set(args[1].toLowerCase(), args.slice(2).join(" "));
        } else if (verb === "unbind") {
          if (args.length >= 2) binds.delete(args[1].toLowerCase());
        } else if (verb === "alias") {
          if (args.length >= 2) aliases.set(args[1], args.slice(2).join(" "));
        } else if (args.length >= 2) {
          cvars.set(verb, args[1]);
        }
      }
    }
  };

  for (const root of roots) run(root, 0);
  return { cvars, binds, aliases };
}

module.exports = { runChain };
