import { createServer } from 'http'
import webpack from 'webpack'
import DevMiddleware from 'webpack-dev-middleware'
import HotMiddleware from 'webpack-hot-middleware'
import { forceUnwrap as u } from '../app/src/lib/fatal-error'

/** Serve the Electron renderer and its hot-reload event stream. */
export function createRendererDevServer(config: webpack.Configuration) {
  const compiler = webpack(config)
  const message = 'Could not find public path from configuration'
  const devMiddleware = DevMiddleware(compiler, {
    publicPath: u(message, u(message, config.output).publicPath),
  })
  const hotMiddleware = HotMiddleware(compiler)
  const server = createServer((req, res) => {
    devMiddleware(req, res, () => {
      hotMiddleware(req, res, () => {
        res.writeHead(404, { 'Content-Type': 'text/plain' })
        res.end('Not found')
      })
    })
  })

  return {
    server,
    async close(): Promise<void> {
      hotMiddleware.close()
      try {
        if (server.listening) {
          await new Promise<void>((resolve, reject) => {
            server.close(error => (error ? reject(error) : resolve()))
          })
        }
      } finally {
        await new Promise<void>((resolve, reject) => {
          devMiddleware.close(error => (error ? reject(error) : resolve()))
        })
      }
    },
  }
}
