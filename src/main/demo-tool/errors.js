"use strict";

// Ошибки инструмента «Демо»: стабильный код для интерфейса (тексты — в
// локалях рендерера) и необязательные подробности без путей пользователя.
class DemoError extends Error {
  constructor(code, message, details) {
    super(message || code);
    this.name = "DemoError";
    this.code = code;
    if (details) this.details = details;
  }
}

function publicError(error, fallback) {
  if (error && error.name === "DemoError") return Object.assign({ code: error.code }, error.details || {});
  const code = error && typeof error.code === "string" ? error.code : "";
  // errno Node (ENOSPC, EACCES…) — не коды контракта: в окно их не отдаём.
  if (code === "ENOSPC") return { code: "NO_SPACE" };
  if (/^[A-Z][A-Z0-9_]+$/.test(code) && !/^E[A-Z0-9]+$/.test(code)) return { code };
  return { code: fallback || "JOB_FAILED" };
}

module.exports = { DemoError, publicError };
