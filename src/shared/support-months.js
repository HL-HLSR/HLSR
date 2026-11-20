"use strict";

// Сколько полных календарных месяцев человек поддерживает HLSR: с момента
// sinceSeconds (unix-секунды) до nowMs. Меньше месяца, будущее и мусор — 0.
// Считается по UTC, чтобы число не зависело от часового пояса.
function supportMonths(sinceSeconds, nowMs) {
  if (!Number.isSafeInteger(sinceSeconds) || sinceSeconds <= 0 || !Number.isFinite(nowMs)) return 0;
  const since = new Date(sinceSeconds * 1000), now = new Date(nowMs);
  let months = (now.getUTCFullYear() - since.getUTCFullYear()) * 12 + now.getUTCMonth() - since.getUTCMonth();
  // День и время месяца ещё не наступили — последний месяц неполный.
  const rest = (date) => ((date.getUTCDate() * 24 + date.getUTCHours()) * 60 + date.getUTCMinutes()) * 60 + date.getUTCSeconds();
  if (rest(now) < rest(since)) months -= 1;
  return Math.max(0, months);
}

module.exports = { supportMonths };
