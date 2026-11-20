'use strict'

const electron = require('electron')
const path = require('path')
const { spawn } = require('child_process')
const webpack = require('webpack')
const WebpackDevServer = require('webpack-dev-server')

const mainConfig = require('./webpack.main.config')
const rendererConfig = require('./webpack.renderer.config')

const PORT = 9080

let electronProcess = null
let manualRestart = false

function log (name, text) {
  const body = String(text).split(/\r?\n/).filter(Boolean).map(line => `  ${line}`).join('\n')
  if (body.trim()) console.log(`[${name}]\n${body}\n`)
}

function logStats (name, stats) {
  log(name, stats.toString({ preset: 'minimal', colors: process.stdout.isTTY }))
}

// Renderer раздаёт webpack-dev-server с горячей заменой компонентов Vue.
async function startRenderer () {
  rendererConfig.mode = 'development'
  const compiler = webpack(rendererConfig)
  compiler.hooks.done.tap('hlsr-dev', stats => logStats('Renderer', stats))

  const server = new WebpackDevServer({
    port: PORT,
    host: '127.0.0.1',
    hot: true,
    static: false,
    // Страница открыта с localhost, и CSP из index.ejs пускает веб-сокет
    // только туда: по умолчанию клиент шёл на 127.0.0.1, CSP его блокировал,
    // и горячая замена не доходила до окна.
    client: { overlay: { warnings: false }, webSocketURL: `ws://localhost:${PORT}/ws` },
    devMiddleware: { stats: 'none' }
  }, compiler)
  await server.start()
}

// Main собирается в режиме watch; после пересборки Electron перезапускается.
function startMain () {
  return new Promise((resolve) => {
    mainConfig.entry.main = [path.join(__dirname, '../src/main/index.dev.js')].concat(mainConfig.entry.main)
    mainConfig.mode = 'development'
    const compiler = webpack(mainConfig)

    compiler.watch({}, (err, stats) => {
      if (err) {
        console.error(err)
        return
      }

      logStats('Main', stats)

      if (electronProcess && electronProcess.kill) {
        manualRestart = true
        process.kill(electronProcess.pid)
        electronProcess = null
        startElectron()

        setTimeout(() => {
          manualRestart = false
        }, 5000)
      }

      resolve()
    })
  })
}

function startElectron () {
  let args = [
    '--inspect=5858',
    path.join(__dirname, '../dist/electron/main.js')
  ]

  if (process.env.npm_execpath && process.env.npm_execpath.endsWith('npm-cli.js')) {
    args = args.concat(process.argv.slice(2))
  }

  // NODE_OPTIONS от webpack Electron не принимает.
  electronProcess = spawn(electron, args, {
    env: Object.assign({}, process.env, { NODE_OPTIONS: '' })
  })

  electronProcess.stdout.on('data', data => log('Electron', data))
  electronProcess.stderr.on('data', data => log('Electron', data))

  electronProcess.on('close', () => {
    if (!manualRestart) process.exit()
  })
}

Promise.all([startRenderer(), startMain()])
  .then(startElectron)
  .catch(err => {
    console.error(err)
    process.exit(1)
  })
