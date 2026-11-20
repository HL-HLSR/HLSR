"use strict";

const { DeliveryError } = require("./errors");

const MAX_DEPTH = 32;

const ESCAPES = {
  '"': '"',
  "\\": "\\",
  n: "\n",
  t: "\t",
  r: "\r",
};

function tokenize(text) {
  const source = String(text == null ? "" : text);
  const tokens = [];
  let index = 0;
  let line = 1;

  const fail = (message) => {
    throw new DeliveryError("LAUNCH_CONFIG_INVALID", `VDF, строка ${line}: ${message}`);
  };

  while (index < source.length) {
    const char = source[index];

    if (char === "\n") {
      line++;
      index++;
      continue;
    }

    if (char === " " || char === "\t" || char === "\r") {
      index++;
      continue;
    }

    if (char === "/" && source[index + 1] === "/") {
      while (index < source.length && source[index] !== "\n") index++;
      continue;
    }

    if (char === "{" || char === "}") {
      tokens.push({ type: char, line });
      index++;
      continue;
    }

    if (char === "[") {
      const end = source.indexOf("]", index);
      if (end === -1) fail("незакрытая условная метка [");
      index = end + 1;
      continue;
    }

    if (char === '"') {
      let value = "";
      index++;

      for (;;) {
        if (index >= source.length) fail("незакрытая строка");

        const current = source[index];

        if (current === '"') {
          index++;
          break;
        }

        if (current === "\\") {
          const next = source[index + 1];

          if (next != null && ESCAPES[next] != null) {
            value += ESCAPES[next];
            index += 2;
            continue;
          }

          value += current;
          index++;
          continue;
        }

        if (current === "\n") line++;

        value += current;
        index++;
      }

      tokens.push({ type: "string", value, line });
      continue;
    }

    let value = "";
    while (index < source.length && !/[\s{}"]/.test(source[index])) {
      if (source[index] === "/" && source[index + 1] === "/") break;
      value += source[index];
      index++;
    }

    if (!value) fail(`неожиданный символ "${char}"`);
    tokens.push({ type: "string", value, line });
  }

  return tokens;
}

function parse(text) {
  const tokens = tokenize(text);
  let position = 0;

  const fail = (token, message) => {
    throw new DeliveryError(
      "LAUNCH_CONFIG_INVALID",
      `VDF, строка ${token ? token.line : "?"}: ${message}`
    );
  };

  const parseObject = (depth) => {
    if (depth > MAX_DEPTH) fail(tokens[position], "слишком глубокая вложенность");

    const result = {};

    for (;;) {
      const token = tokens[position];

      if (!token) {
        if (depth === 0) return result;
        fail(null, "неожиданный конец файла: не хватает }");
      }

      if (token.type === "}") {
        if (depth === 0) fail(token, "лишняя закрывающая скобка");
        position++;
        return result;
      }

      if (token.type !== "string") fail(token, `ожидался ключ, а не "${token.type}"`);

      const key = token.value;
      position++;

      const next = tokens[position];
      if (!next) fail(token, `у ключа "${key}" нет значения`);

      if (next.type === "string") {
        result[key] = next.value;
        position++;
        continue;
      }

      if (next.type === "{") {
        position++;
        result[key] = parseObject(depth + 1);
        continue;
      }

      fail(next, `у ключа "${key}" вместо значения "${next.type}"`);
    }
  };

  return parseObject(0);
}

function getInsensitive(object, key) {
  if (!object || typeof object !== "object") return undefined;

  const lower = String(key).toLowerCase();
  for (const candidate of Object.keys(object))
    if (candidate.toLowerCase() === lower) return object[candidate];

  return undefined;
}

module.exports = { parse, tokenize, getInsensitive, MAX_DEPTH };
