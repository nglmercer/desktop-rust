import { vi } from 'vitest'
import {
  appendFileSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from 'node:fs'

/**
 * Minimal Node-backed implementation of the host filesystem bridge for tests
 * of renderer code that explicitly uses `web/fs`. Production uses Rust; here
 * the same machine-local semantics are supplied through Node.
 */
export function installNodeFileBridge() {
  vi.stubGlobal('__desktop', {
    invoke: (channel: string, options: any): Promise<unknown> => {
      if (!channel.startsWith('desktop.io.fs-')) {
        return Promise.reject(new Error(`unexpected ${channel}`))
      }
      try {
        switch (channel.slice('desktop.io.fs-'.length)) {
          case 'write-file': {
            const bytes = Buffer.from(options.data ?? '', 'base64')
            if (options.append === true) {
              appendFileSync(options.path, bytes)
            } else {
              writeFileSync(options.path, bytes, {
                flag: options.exclusive === true ? 'wx' : 'w',
                mode: options.mode,
              })
            }
            return Promise.resolve(null)
          }
          case 'rm':
            rmSync(options.path, {
              recursive: options.recursive === true,
              force: options.force === true,
            })
            return Promise.resolve(null)
          case 'mkdir':
            mkdirSync(options.path, { recursive: options.recursive === true })
            return Promise.resolve(null)
          case 'copy-file':
            copyFileSync(options.path, options.to)
            return Promise.resolve(null)
          case 'mkdtemp':
            return Promise.resolve(mkdtempSync(options.prefix))
          default:
            return Promise.reject(
              new Error(
                `unexpected fs ${channel.slice('desktop.io.fs-'.length)}`
              )
            )
        }
      } catch (error) {
        return Promise.reject(error)
      }
    },
    invokeSync: () => {
      throw new Error(
        'synchronous filesystem bridge is unavailable in this test'
      )
    },
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  })
}

export function uninstallNodeFileBridge() {
  vi.unstubAllGlobals()
}
