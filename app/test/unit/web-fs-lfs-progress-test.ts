import { afterEach, describe, expect, it, vi } from 'vitest'
import { existsSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { createLFSProgressFile } from '../../src/lib/progress/lfs'

// The production LFS progress file creation path, driven against a small fake
// of the host filesystem bridge. Writes use real wx semantics so the exclusive
// create contract is exercised rather than only the request shape.
function installBridge() {
  const writes: Array<{ path: string; exclusive?: boolean }> = []
  vi.stubGlobal('__desktop', {
    invoke: (
      channel: string,
      options: { path: string; data: string; exclusive?: boolean }
    ) => {
      if (channel !== 'desktop.io.fs-write-file') {
        return Promise.reject(new Error(`unexpected ${channel}`))
      }
      writes.push({ path: options.path, exclusive: options.exclusive })
      try {
        writeFileSync(options.path, Buffer.from(options.data, 'base64'), {
          flag: options.exclusive === true ? 'wx' : 'w',
        })
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
  return writes
}

describe('LFS progress file creation through the host filesystem', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('creates unique empty files exclusively in the host temporary directory', async () => {
    const writes = installBridge()
    const first = await createLFSProgressFile()
    const second = await createLFSProgressFile()
    try {
      expect(first).not.toBe(second)
      expect(first.startsWith(tmpdir())).toBe(true)
      expect(existsSync(first)).toBe(true)
      expect(existsSync(second)).toBe(true)
      expect(writes).toEqual([
        { path: first, exclusive: true },
        { path: second, exclusive: true },
      ])
    } finally {
      rmSync(first, { force: true })
      rmSync(second, { force: true })
    }
  })
})
