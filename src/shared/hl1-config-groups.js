"use strict";

// Находки проверки конфига HL1 (src/shared/hl1-config-check.js) для окна (R22):
// одна группа на имя команды и правило, внутри — места. Модуль лёгкий:
// таблица разрешённых команд в окно не попадает.

const LEVEL_ORDER = { error: 0, warning: 1, info: 2 };

// Нормализованное правило → строка локали.
const RULE_KEYS = {
  banned: "#L_HC_RULE_BANNED",
  unknown: "#L_HC_RULE_UNKNOWN",
  scripted: "#L_HC_RULE_SCRIPTED",
  "scripted-only": "#L_HC_RULE_SCRIPTED_ONLY",
  "no-script-seq": "#L_HC_RULE_NO_SEQ",
  category: "#L_HC_RULE_CATEGORY",
  thirdperson: "#L_HC_RULE_THIRDPERSON",
  steam: "#L_HC_RULE_STEAM",
  values: "#L_HC_RULE_VALUES",
  skill: "#L_HC_RULE_SKILL",
  "run-start": "#L_HC_RULE_RUN_START",
  missing: "#L_HC_RULE_MISSING",
};

// Сначала ошибки, затем предупреждения и справки; внутри уровня больше мест — выше.
function groupFindings(findings) {
  const groups = new Map();
  for (const finding of findings || []) {
    const name = String(finding.command).split(/\s+/)[0].toLowerCase();
    const key = `${name} ${finding.rule}`;
    let group = groups.get(key);
    if (!group) {
      group = { key, name, rule: finding.rule, level: finding.level, note: finding.note || "", places: [] };
      groups.set(key, group);
    }
    group.places.push({
      file: finding.file, path: finding.path || null, line: finding.line, command: finding.command, context: finding.context,
      trail: finding.trail || [],
    });
  }
  return [...groups.values()].sort((a, b) =>
    LEVEL_ORDER[a.level] - LEVEL_ORDER[b.level] || b.places.length - a.places.length || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
}

// Хвост пояснения после двоеточия: допустимые значения или значение по умолчанию
// (rule values/skill) — данные, не текст, их показываем рядом с переведённой строкой.
function ruleDetail(group) {
  if (group.rule !== "values" && group.rule !== "skill") return "";
  const at = group.note.lastIndexOf(": ");
  return at === -1 ? "" : group.note.slice(at + 2);
}

module.exports = { groupFindings, ruleDetail, RULE_KEYS };
