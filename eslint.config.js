"use strict";

const js = require("@eslint/js");
const globals = require("globals");
const vueParser = require("vue-eslint-parser");
const html = require("eslint-plugin-html");

module.exports = [
  {
    // Сборка, зависимости и выходные каталоги
    ignores: ["dist/**", "build/**", "node_modules/**", "release/**"],
  },
  js.configs.recommended,
  {
    files: ["src/**/*.js", "src/**/*.vue", "src/**/*.html"],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        ...globals.browser,
        ...globals.node,
        __static: "readonly",
      },
    },
    rules: {
      "no-empty": ["error", { allowEmptyCatch: true }],
      "no-unused-vars": ["error", { args: "none", ignoreRestSiblings: true, caughtErrors: "none" }],
      "no-debugger": process.env.NODE_ENV === "production" ? "error" : "off",
      // Защитная инициализация перед try (`let entries = []`) — принятый
      // в проекте приём; правило ESLint 10 считает её лишним присваиванием.
      "no-useless-assignment": "off",
    },
  },
  {
    files: ["src/**/*.vue"],
    languageOptions: { parser: vueParser },
  },
  {
    files: ["src/**/*.html"],
    plugins: { html },
  },
];
