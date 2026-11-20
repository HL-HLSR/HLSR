"use strict";

// Судьи тестировщика.
// - Jev (TypeSafe System One): текстовое состояние → типизированные ответы.
//   Ключ: TYPESAFE_API_KEY.
// - Clef (Cloudflare Workers AI): то же API, но умеет смотреть скриншоты.
//   Ключи: CF_ACCOUNT_ID и CF_API_TOKEN.
// Оба принимают вопросы noul/choice/score и возвращают вероятности; решение
// «баг или нет» принимает код по порогам, а не модель.

const JEV_URL = "https://api.typesafe.ai/v1/systemone";
const MAX_QUESTIONS = 64;

class Limiter {
  constructor(limit) {
    this.limit = limit;
    this.active = 0;
    this.queue = [];
  }

  async run(task) {
    if (this.active >= this.limit) await new Promise((resolve) => this.queue.push(resolve));
    this.active++;
    try {
      return await task();
    } finally {
      this.active--;
      const next = this.queue.shift();
      if (next) next();
    }
  }
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function postJson(url, headers, body, { attempts = 4, timeoutMs = 90000 } = {}) {
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt++) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      const response = await fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...headers },
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      const text = await response.text();
      let data = null;
      try {
        data = JSON.parse(text);
      } catch (error) {
        data = null;
      }
      if (response.ok) return data;
      lastError = new Error(`HTTP ${response.status}: ${text.slice(0, 300)}`);
      // 4xx кроме лимитов повторять бессмысленно: запрос сформирован неверно.
      if (response.status < 500 && response.status !== 429 && response.status !== 408) throw lastError;
      const retryAfter = Number(response.headers.get("retry-after"));
      await delay(retryAfter > 0 ? retryAfter * 1000 : 1500 * attempt * attempt);
    } catch (error) {
      if (error === lastError) throw error;
      lastError = error.name === "AbortError" ? new Error(`таймаут ${timeoutMs} мс`) : error;
      await delay(1500 * attempt * attempt);
    } finally {
      clearTimeout(timer);
    }
  }
  throw lastError;
}

function chunkQuestions(questions) {
  const entries = Object.entries(questions);
  const chunks = [];
  for (let i = 0; i < entries.length; i += MAX_QUESTIONS) chunks.push(Object.fromEntries(entries.slice(i, i + MAX_QUESTIONS)));
  return chunks;
}

class Judges {
  constructor({ env = process.env, jevModel = "jev-latest", clefModel = "clef", concurrency = 4, offline = false } = {}) {
    this.jevKey = env.TYPESAFE_API_KEY || "";
    this.cfAccount = env.CF_ACCOUNT_ID || "";
    this.cfToken = env.CF_API_TOKEN || "";
    this.jevModel = jevModel;
    this.clefModel = clefModel;
    this.offline = offline;
    this.limiter = new Limiter(concurrency);
    this.usage = { jev: { calls: 0, input: 0, output: 0, errors: 0 }, clef: { calls: 0, input: 0, output: 0, errors: 0 } };
  }

  get hasJev() {
    return !this.offline && !!this.jevKey;
  }

  get hasClef() {
    return !this.offline && !!(this.cfAccount && this.cfToken);
  }

  // Текстовый судья. Возвращает { answers } или { error }.
  async jev(state, questions) {
    if (!this.hasJev) return { skipped: "нет TYPESAFE_API_KEY или включён --offline" };
    const answers = {};
    for (const chunk of chunkQuestions(questions)) {
      try {
        const data = await this.limiter.run(() =>
          postJson(JEV_URL, { Authorization: `Bearer ${this.jevKey}` }, { model: this.jevModel, state, questions: chunk })
        );
        this.usage.jev.calls++;
        this.usage.jev.input += (data.usage && data.usage.input_tokens) || 0;
        this.usage.jev.output += (data.usage && data.usage.output_tokens) || 0;
        Object.assign(answers, data.answers);
      } catch (error) {
        this.usage.jev.errors++;
        return { error: error.message, answers };
      }
    }
    return { answers };
  }

  // Визуальный судья: до 4 JPEG-скриншотов плюс текстовое состояние.
  async clef(images, state, questions) {
    if (!this.hasClef) return { skipped: "нет CF_ACCOUNT_ID/CF_API_TOKEN или включён --offline" };
    const url = `https://api.cloudflare.com/client/v4/accounts/${this.cfAccount}/ai/run/@cf/cloudflare/${this.clefModel}`;
    const encoded = images.slice(0, 4).map((buffer) => `data:image/jpeg;base64,${buffer.toString("base64")}`);
    const answers = {};
    for (const chunk of chunkQuestions(questions)) {
      try {
        const data = await this.limiter.run(() =>
          postJson(url, { Authorization: `Bearer ${this.cfToken}` }, { model: this.clefModel, images: encoded, state, questions: chunk })
        );
        if (!data || data.success === false) throw new Error(JSON.stringify((data && data.errors) || data).slice(0, 300));
        const result = data.result || {};
        this.usage.clef.calls++;
        this.usage.clef.input += (result.usage && result.usage.input_tokens) || 0;
        this.usage.clef.output += (result.usage && result.usage.output_tokens) || 0;
        Object.assign(answers, result.answers);
      } catch (error) {
        this.usage.clef.errors++;
        return { error: error.message, answers };
      }
    }
    return { answers };
  }
}

// Ответ модели → число: noul — вероятность «да», choice — выбранный вариант.
function noul(answers, id) {
  const answer = answers && answers[id];
  return answer && typeof answer.noul === "number" ? answer.noul : null;
}

function choice(answers, id) {
  const answer = answers && answers[id];
  return answer ? { value: answer.choice, confidence: answer.confidence, probabilities: answer.probabilities } : null;
}

module.exports = { Judges, noul, choice };
