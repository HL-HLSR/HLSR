"use strict";

const fs = require("fs");
const path = require("path");

// Дни подряд, в которые открывали лаунчер или запускали игру, — для серий
// («Неделя в Мезе» и дальше) и сводки профиля. День считается по местному
// времени: игрок думает календарными днями, а не сутками по UTC. Пропуск
// дня начинает серию с единицы; best — рекорд.
const FILE_NAME = "launch-days.json";
const GOALS = [["week-streak", 7], ["hard-worker", 14], ["day-after-day", 30], ["keep-the-flame", 100]];

function dayOf(time) {
  const date = new Date(time);
  const pad = (n) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function previousDay(day) {
  const [year, month, date] = day.split("-").map(Number);
  return dayOf(new Date(year, month - 1, date - 1).getTime());
}

function read(file) {
  try {
    const data = JSON.parse(fs.readFileSync(file, "utf8"));
    if (data && /^\d{4}-\d{2}-\d{2}$/.test(data.last) && Number.isSafeInteger(data.streak) && data.streak > 0) return data;
  } catch (error) {
    /* нет файла или он испорчен — начинаем заново */
  }
  return null;
}

// Отмечает день и возвращает, сколько дней подряд набралось.
function record(dir, now) {
  const file = path.join(dir, FILE_NAME);
  const today = dayOf(now);
  const saved = read(file);

  // Сохранённый день позже сегодняшнего (часы перевели назад, перелёт на
  // запад) — тот же день: серия не обнуляется, last не переписывается.
  // Строки ГГГГ-ММ-ДД сравниваются как даты.
  if (saved && saved.last >= today) return saved.streak;

  const streak = saved && saved.last === previousDay(today) ? saved.streak + 1 : 1;
  try {
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(file + ".tmp", JSON.stringify({ last: today, streak, best: Math.max(streak, bestOf(saved)) }));
    fs.renameSync(file + ".tmp", file);
  } catch (error) {
    /* без записи серия просто начнётся заново */
  }
  return streak;
}

// Рекорд; в файле до 0.4.9 его нет — тогда это текущая серия.
function bestOf(saved) {
  if (!saved) return 0;
  return Math.max(saved.streak, Number.isSafeInteger(saved.best) ? saved.best : 0);
}

// Для сводки: серия, если последний день — вчера или позже, и рекорд.
function stats(dir, now) {
  const saved = read(path.join(dir, FILE_NAME));
  if (!saved) return null;
  return { days: saved.last >= previousDay(dayOf(now)) ? saved.streak : 0, best: bestOf(saved) };
}

module.exports = { record, stats, dayOf, previousDay, FILE_NAME, GOALS };
