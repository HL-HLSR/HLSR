'use strict'

process.env.NODE_ENV = 'production'

const fs = require('fs')
const path = require('path')
const webpack = require('webpack')

const root = path.join(__dirname, '..')

// Байткод main-процесса (scripts/compile-main-bytecode.js). HLSR_BYTECODE=0
// оставляет в dist/electron обычный main.js: для отладки сборки.
const bytecode = process.env.HLSR_BYTECODE !== '0'

if (process.env.BUILD_TARGET === 'clean') clean()
else build()

// Очищает результаты electron-builder, но оставляет ресурсы сборки из git.
function clean () {
  const buildDir = path.join(root, 'build')
  for (const entry of fs.existsSync(buildDir) ? fs.readdirSync(buildDir) : []) {
    if (entry === 'icons') continue
    fs.rmSync(path.join(buildDir, entry), { recursive: true, force: true })
  }
  console.log('build/ очищена')
}

async function build () {
  // Компиляции байткода нужен Electron с дисплеем: без него на Linux
  // сборка перезапускается под xvfb-run, пока webpack ещё не начался.
  if (bytecode) require('../scripts/compile-main-bytecode').ensureDisplay()

  const out = path.join(root, 'dist/electron')
  for (const entry of fs.existsSync(out) ? fs.readdirSync(out) : []) {
    if (entry !== '.gitkeep') fs.rmSync(path.join(out, entry), { recursive: true, force: true })
  }

  // Конфиги читают NODE_ENV при загрузке, поэтому подключаем их после его установки.
  const configs = {
    main: require('./webpack.main.config'),
    renderer: require('./webpack.renderer.config')
  }

  const results = await Promise.allSettled(Object.entries(configs).map(([name, config]) => pack(name, config)))
  const failed = results.filter(result => result.status === 'rejected')
  for (const result of results) {
    console.log(result.status === 'fulfilled' ? result.value : result.reason)
  }
  if (failed.length) process.exit(1)

  // Апдейтер Windows едет в app.asar рядом с main-бандлом
  // (src/main/launcher-update/updater-mode.js).
  if (require('../scripts/lib/build-target').targetPlatform() === 'win32') {
    try {
      console.log(await require('../scripts/lib/updater-bundle').build(path.join(out, 'updater')))
    } catch (error) {
      console.error(`апдейтер: сборка не удалась: ${error.message}`)
      process.exit(1)
    }
  }

  if (bytecode) {
    try {
      const result = await require('../scripts/compile-main-bytecode').compileMain()
      console.log(`main: байткод ${(result.bytecode / 1048576).toFixed(1)} МБ, исходный бандл в ${path.relative(root, result.symbols)}`)
    } catch (error) {
      console.error(`main: компиляция байткода не удалась: ${error.message}`)
      process.exit(1)
    }
  }
}

function pack (name, config) {
  return new Promise((resolve, reject) => {
    webpack(config, (err, stats) => {
      if (err) return reject(`${name}: ${err.stack || err}`)
      const text = stats.toString({ preset: 'errors-warnings', colors: process.stdout.isTTY })
      if (stats.hasErrors()) reject(`${name}: сборка не удалась\n${text}`)
      else resolve(`${name}: собран за ${stats.endTime - stats.startTime} мс${text ? '\n' + text : ''}`)
    })
  })
}
