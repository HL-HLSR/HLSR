"use strict";
const fs = require("fs");
const path = require("path");
const { minifyDir, summary } = require("./minify-js");
const root = path.resolve(__dirname, "..");
const output = path.join(root, "build/installer");
fs.rmSync(output, { recursive: true, force: true });
fs.mkdirSync(output, { recursive: true });
for (const name of ["index.html", "installer.js", "preload.js", "style.css"])
  fs.copyFileSync(path.join(root, "src/installer", name), path.join(output, name));
fs.copyFileSync(path.join(root, "src/renderer/assets/css/launcher.css"), path.join(output, "launcher.css"));
fs.cpSync(path.join(root, "src/renderer/assets/fonts"), path.join(output, "fonts"), { recursive: true });

// build/installer нужен только сборкам (Windows и AppImage-установщику),
// поэтому окно установщика уходит туда уже сжатым.
minifyDir(output, { browserScripts: ["installer.js"] }).then(
  (results) => console.log(summary("окно установщика", results)),
  (error) => {
    console.error(error.message);
    process.exitCode = 1;
  }
);
