"use strict";

// Отчёт приёмки: строки «ok / FAIL / skip», код выхода 1 при любой неудаче. skip — только то, что
// на этой машине сделать нельзя (причина в строке): Linux-часть на Windows, настоящий рантайм
// AppImage без Linux. Пропуск не скрывается: итог считает их отдельно.

class Report {
  constructor(write = (line) => process.stdout.write(`${line}\n`)) {
    this.write = write;
    this.results = [];
    this.section = "";
  }

  begin(title) {
    this.section = title;
    this.write(`== ${title}`);
  }

  add(status, name, detail) {
    const entry = { section: this.section, status, name, detail: detail || "" };
    this.results.push(entry);
    const label = { ok: "ok   ", fail: "FAIL ", skip: "skip " }[status];
    this.write(`${label} ${name}${detail ? ` — ${detail}` : ""}`);
  }

  ok(name, detail) { this.add("ok", name, detail); }
  fail(name, detail) { this.add("fail", name, detail); }
  skip(name, reason) { this.add("skip", name, reason); }

  /**
   * Выполнить проверку. fn возвращает строку-деталь, { skip: "причина" } или ничего; исключение —
   * неудача (текст ошибки в строке).
   */
  async step(name, fn) {
    try {
      const result = await fn();
      if (result && typeof result === "object" && result.skip) this.skip(name, result.skip);
      else this.ok(name, typeof result === "string" ? result : "");
    } catch (error) {
      this.fail(name, error && error.message ? error.message : String(error));
    }
  }

  get failed() { return this.results.filter((entry) => entry.status === "fail"); }
  get skipped() { return this.results.filter((entry) => entry.status === "skip"); }

  summary() {
    const count = (status) => this.results.filter((entry) => entry.status === status).length;
    const line = `итого: ok ${count("ok")}, FAIL ${count("fail")}, skip ${count("skip")}`;
    this.write(line);
    return line;
  }
}

module.exports = { Report };
