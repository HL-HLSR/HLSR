'use strict'

const path = require('path')
const { dependencies, optionalDependencies } = require('../package.json')
const webpack = require('webpack')

// Babel не используется: main работает в Node.js из Electron 45. Модули
// src/main/** и src/shared/** написаны на CommonJS, потому что selftest'ы
// подключают их через require() в обычном Node без сборки.

const mainConfig = {
  mode: process.env.NODE_ENV === 'production' ? 'production' : 'development',
  devtool: false,
  entry: {
    main: path.join(__dirname, '../src/main/entry.js'),
    preload: path.join(__dirname, '../src/main/preload.js')
  },
  externals: [
    ...Object.keys(dependencies || {}),
    // Development-only ESM packages are loaded by Electron's Node.js loader.
    'electron-debug',
    'electron-devtools-installer',
    // hlsr-native есть только в Windows-сборке; в Linux его нет в node_modules
    ...Object.keys(optionalDependencies || {})
  ],
  module: {
    rules: [
      {
        test: /\.node$/,
        use: 'node-loader'
      }
    ]
  },
  node: {
    // preload.js is emitted beside main.js in development as well as production.
    // Electron requires an absolute path, not webpack's source-directory mock.
    __dirname: false,
    __filename: false
  },
  output: {
    filename: '[name].js',
    library: { type: 'commonjs2' },
    path: path.join(__dirname, '../dist/electron')
  },
  plugins: [],
  optimization: {
    emitOnErrors: false,
    // Минификация только в production (ниже): комментарии убираются,
    // локальные имена сокращаются.
    minimize: false
  },
  resolve: {
    extensions: ['.js', '.json', '.node']
  },
  target: 'electron-main'
}

/**
 * Adjust mainConfig for development settings
 */

/**
 * Adjust mainConfig for production settings
 *
 * HLSR_BUILD_CHANNEL вшивается в бандл КОНСТАНТОЙ, а не читается из окружения
 * пользователя. Только сборка с HLSR_BUILD_CHANNEL=qa разрешает подменить feed
 * обновления лаунчера на локальный loopback; в обычной production-сборке
 * условие превращается в false и весь QA-код становится недостижимым.
 */
if (process.env.NODE_ENV === 'production') {
  const buildChannel = process.env.HLSR_BUILD_CHANNEL === 'qa' ? 'qa' : 'production'

  // Бандл в app.asar не должен читаться как исходники: Terser убирает
  // комментарии и сокращает локальные имена. Имена функций и классов
  // остаются — они нужны в логах и отчётах о сбоях (стеки, имена ошибок).
  mainConfig.optimization.minimize = true
  mainConfig.optimization.minimizeOptions = {
    javascript: {
      compress: { passes: 2 },
      keep_classnames: true,
      keep_fnames: true,
      format: { comments: false }
    }
  }

  // Ключ аттестации сборки (src/main/security/attestation.js) есть только у
  // production-сборки канала production. Запись вшивается КОНСТАНТОЙ, как и
  // канал; публичная половина уходит в app.asar маркером attest.json.
  const attestKeys = require('../scripts/lib/attest-keys')
  const target = require('../scripts/lib/build-target').targetPlatform()
  const attest = buildChannel === 'production' && !attestKeys.disabled(process.env, target)
    ? attestKeys.forBuild({ platform: target, version: require('../package.json').version })
    : null
  // Официальной сборке нужны адрес онлайна и приложение Discord: без них она молча стала бы
  // неофициальной (src/main/security/official.js).
  if (attest && (!process.env.HLSR_API_HOST || !process.env.HLSR_DISCORD_APP_ID))
    throw new Error('Сборка выпуска с ключом аттестации: задайте HLSR_API_HOST и HLSR_DISCORD_APP_ID')

  mainConfig.plugins.push(
    new webpack.DefinePlugin({
      'process.env.NODE_ENV': '"production"',
      'process.env.HLSR_BUILD_CHANNEL': JSON.stringify(buildChannel),
      'process.env.HLSR_ATTEST_SEAL': JSON.stringify(attest ? attest.seal : ''),
      // Адрес онлайна и приложение Discord есть только у официальной сборки (с ключом
      // аттестации): их задаёт окружение сборки выпуска, в исходниках значений нет.
      'process.env.HLSR_API_HOST': JSON.stringify(attest ? (process.env.HLSR_API_HOST || '') : ''),
      'process.env.HLSR_DISCORD_APP_ID': JSON.stringify(attest ? (process.env.HLSR_DISCORD_APP_ID || '') : '')
    })
  )

  if (attest) {
    mainConfig.plugins.push({
      apply (compiler) {
        compiler.hooks.thisCompilation.tap('HlsrAttestMarker', (compilation) => {
          compilation.hooks.processAssets.tap({
            name: 'HlsrAttestMarker',
            stage: webpack.Compilation.PROCESS_ASSETS_STAGE_ADDITIONAL
          }, () => {
            compilation.emitAsset(attestKeys.MARKER, new webpack.sources.RawSource(JSON.stringify(attest.marker) + '\n'))
          })
        })
      }
    })
  }
}

module.exports = mainConfig
