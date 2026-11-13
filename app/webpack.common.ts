import * as path from 'path'
import HtmlWebpackPlugin from 'html-webpack-plugin'
import webpack from 'webpack'
import merge from 'webpack-merge'
import { getReplacements } from './app-info'
import { getDistArchitecture } from '../script/dist-info'
import { getKoffiWebpackConfig } from '../script/webpack-koffi'
import { packagedExternals } from './build-externals'

const outputDir = 'out'
export const replacements = getReplacements()

const commonConfig: webpack.Configuration = {
  optimization: {
    emitOnErrors: false,
  },
  externals: packagedExternals,
  output: {
    filename: '[name].js',
    path: path.resolve(__dirname, '..', outputDir),
    library: {
      name: '[name]',
      type: 'commonjs2',
    },
  },
  module: {
    rules: [
      {
        test: /\.tsx?$/,
        include: path.resolve(__dirname, 'src'),
        use: [
          {
            loader: 'ts-loader',
          },
        ],
        exclude: /node_modules/,
      },
      {
        test: /\.node$/,
        loader: 'awesome-node-loader',
        options: {
          name: '[name].[ext]',
        },
      },
    ],
  },
  resolve: {
    extensions: ['.js', '.ts', '.tsx'],
  },
  node: {
    __dirname: false,
    __filename: false,
  },
}

export const main = merge({}, commonConfig, {
  entry: { main: path.resolve(__dirname, 'src/main-process/main') },
  target: 'electron-main',
  plugins: [
    new webpack.DefinePlugin(
      Object.assign({}, replacements, {
        __PROCESS_KIND__: JSON.stringify('main'),
      })
    ),
  ],
})

export const renderer = merge(
  {},
  commonConfig,
  getKoffiWebpackConfig(
    path.resolve(__dirname, 'node_modules'),
    process.platform,
    getDistArchitecture()
  ),
  {
    entry: { renderer: path.resolve(__dirname, 'src/ui/index') },
    target: 'electron-renderer',
    module: {
      rules: [
        {
          test: /\.(jpe?g|png|gif|ico)$/,
          use: ['file?name=[path][name].[ext]'],
        },
        {
          test: /\.cmd$/,
          type: 'asset/resource',
        },
      ],
    },
    plugins: [
      new HtmlWebpackPlugin({
        template: path.join(__dirname, 'static', 'index.html'),
        chunks: ['renderer'],
      }),
      new webpack.NormalModuleReplacementPlugin(
        /^vscode-jsonrpc$/,
        resource => {
          resource.request = 'vscode-jsonrpc/lib/node/main.js'
        }
      ),
      new webpack.NormalModuleReplacementPlugin(
        /vscode-jsonrpc[\\/]node(\.js)?$/,
        resource => {
          resource.request = 'vscode-jsonrpc/lib/node/main.js'
        }
      ),
      new webpack.DefinePlugin(
        Object.assign({}, replacements, {
          __PROCESS_KIND__: JSON.stringify('ui'),
        })
      ),
    ],
    resolve: {
      // Prevent the renderer from using browser-specific versions of modules
      aliasFields: [],
    },
  }
)

export const crash = merge({}, commonConfig, {
  entry: { crash: path.resolve(__dirname, 'src/crash/index') },
  target: 'electron-renderer',
  plugins: [
    new HtmlWebpackPlugin({
      title: 'GitHub Desktop',
      filename: 'crash.html',
      chunks: ['crash'],
    }),
    new webpack.DefinePlugin(
      Object.assign({}, replacements, {
        __PROCESS_KIND__: JSON.stringify('crash'),
      })
    ),
  ],
})
