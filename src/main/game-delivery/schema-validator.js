"use strict";

// Общая сборка валидатора JSON Schema для манифестов и каналов.
// В ajv 8 форматы (date-time, uri) живут в отдельном ajv-formats,
// а путь к ошибочному полю называется instancePath.

function compileSchema(schema) {
  const Ajv = require("ajv");
  const addFormats = require("ajv-formats");
  const ajv = new Ajv({ allErrors: true, verbose: true, strict: false });
  addFormats(ajv);
  return ajv.compile(schema);
}

function describeErrors(errors) {
  return (errors || []).map((err) => `${err.instancePath || "/"} ${err.message}`);
}

module.exports = { compileSchema, describeErrors };
