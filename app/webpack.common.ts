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
    // React 16.14 provides this entry without an exports map. ESM packages
    // such as react-confetti otherwise require an explicit file extension.
    alias: {
      '@desktop-assets': path.resolve(__dirname, 'static/common'),
      'react/jsx-runtime$': require.resolve('react/jsx-runtime', {
        paths: [path.resolve(__dirname, 'node_modules')],
      }),
    },
  },
  node: {
    __dirname: false,
    __filename: false,
  },
}

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
