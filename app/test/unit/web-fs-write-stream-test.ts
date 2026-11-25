// @vitest-environment node
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { appendFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { Readable } from 'node:stream'
import { pipeline } from 'node:stream/promises'
import { createWriteStream } from '../../src/web/fs'

function installBridge() {
  vi.stubGlobal('__desktop', {
    invoke: (
      channel: string,
      options: {
        path: string
        data: string
        append?: boolean
        exclusive?: boolean
      }
    ) => {
      if (channel !== 'desktop.io.fs-write-file') {
        return Promise.reject(new Error(`unexpected ${channel}`))
      }
      const bytes = Buffer.from(options.data, 'base64')
      try {
        if (options.append === true) {
          appendFileSync(options.path, bytes, {
            flag: options.exclusive === true ? 'ax' : 'a',
          })
        } else {
          writeFileSync(options.path, bytes, {
            flag: options.exclusive === true ? 'wx' : 'w',
          })
        }
        return Promise.resolve(null)
      } catch (error) {
        return Promise.reject(error)
      }
    },
    invokeSync: () => {
      throw new Error('unexpected synchronous call')
    },
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  })
}

describe('web fs write streams', () => {
  let directory: string | undefined

  afterEach(() => {
    vi.unstubAllGlobals()
    if (directory !== undefined) {
      rmSync(directory, { recursive: true, force: true })
      directory = undefined
    }
  })

  it('pipes chunks through host file appends', async () => {
    installBridge()
    directory = mkdtempSync(join(tmpdir(), 'desktop-web-fs-write-'))
    const target = join(directory, 'input')
    writeFileSync(target, 'old')

    await pipeline(
      Readable.from([Buffer.from('first-'), 'second']),
      createWriteStream(target)
    )

    expect(readFileSync(target, 'utf8')).toBe('first-second')
  })

  it('creates an empty destination when the input ends without chunks', async () => {
    installBridge()
    directory = mkdtempSync(join(tmpdir(), 'desktop-web-fs-write-'))
    const target = join(directory, 'empty')

    await pipeline(Readable.from([]), createWriteStream(target))

    expect(readFileSync(target, 'utf8')).toBe('')
  })
})
