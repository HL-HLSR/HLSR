'use strict'

const path = require('path')
const webpack = require('webpack')

const MiniCssExtractPlugin = require('mini-css-extract-plugin')
const HtmlWebpackPlugin = require('html-webpack-plugin')
const { VueLoaderPlugin } = require('vue-loader')

const production = process.env.NODE_ENV === 'production'

// В production стили выносятся в styles.css, в разработке их вставляет
// style-loader, чтобы работала горячая замена.
const styleLoader = production ? MiniCssExtractPlugin.loader : 'style-loader'

// Babel не нужен: renderer исполняется только в Chromium из Electron 45,
// который понимает весь синтаксис исходников.
const rendererConfig = {
  mode: production ? 'production' : 'development',
  devtool: production ? false : 'eval-cheap-module-source-map',
  entry: {
    renderer: path.join(__dirname, '../src/renderer/main.js')
  },
  module: {
    rules: [
      {
        test: /\.scss$/,
        use: [styleLoader, 'css-loader', 'sass-loader']
      },
      {
        test: /\.css$/,
        use: [styleLoader, 'css-loader']
      },
      {
        test: /\.vue$/,
        loader: 'vue-loader'
      },
      {
        test: /\.(png|jpe?g|gif|svg|webp)(\?.*)?$/,
        type: 'asset',
        parser: { dataUrlCondition: { maxSize: 10000 } },
        generator: { filename: 'imgs/[name]-[contenthash:8][ext]' }
      },
      {
        test: /\.(mp4|webm|ogg|mp3|wav|flac|aac)(\?.*)?$/,
        // Всегда файлом: CSP (index.ejs) не пускает data: в media-src.
        type: 'asset/resource',
        generator: { filename: 'media/[name]-[contenthash:8][ext]' }
      },
      {
        test: /\.(woff2?|eot|ttf|otf)(\?.*)?$/,
        type: 'asset',
        parser: { dataUrlCondition: { maxSize: 10000 } },
        generator: { filename: 'fonts/[name]-[contenthash:8][ext]' }
      }
    ]
  },
  plugins: [
    new VueLoaderPlugin(),
    new MiniCssExtractPlugin({ filename: 'styles.css' }),
    new webpack.DefinePlugin({
      'process.env.WEBPACK_DEV_SERVER': JSON.stringify(process.env.WEBPACK_DEV_SERVER || ''),
      __VUE_OPTIONS_API__: 'true',
      __VUE_PROD_DEVTOOLS__: 'false',
      __VUE_PROD_HYDRATION_MISMATCH_DETAILS__: 'false'
    }),
    new HtmlWebpackPlugin({
      filename: 'index.html',
      template: path.resolve(__dirname, '../src/index.ejs'),
      // В разработке CSP разрешает eval для source map и сокет dev-сервера.
      isDevelopment: !production,
      minify: production && {
        collapseWhitespace: true,
        removeAttributeQuotes: true,
        removeComments: true
      }
    })
  ],
  optimization: {
    emitOnErrors: false
  },
  performance: {
    hints: false
  },
  output: {
    filename: '[name].js',
    path: path.join(__dirname, '../dist/electron'),
    // main.js и preload.js лежат в той же папке; renderer их не трогает.
    clean: false
  },
  resolve: {
    alias: {
      '@': path.join(__dirname, '../src/renderer'),
      // utils/Store.js общий с main. В renderer он работает через мост preload,
      // а ветка с прямым доступом к файлам не выполняется.
      electron: false,
      'electron-log': false,
      fs: false,
      path: false
    },
    extensions: ['.js', '.vue', '.json', '.css']
  },
  // Renderer работает в песочнице без доступа к Node.js.
  target: 'web'
}

module.exports = rendererConfig
