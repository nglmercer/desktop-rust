import { afterEach, describe, expect, it, vi } from 'vitest'
import { closeSync, existsSync, openSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { createLFSProgressFile } from '../../src/lib/progress/lfs'

// The production LFS progress file creation path, driven against a small fake
// of the host filesystem bridge. Writes use real wx semantics so the exclusive
// create contract is exercised rather than only the request shape.
function installBridge() {
  const openings: Array<{ path: string; flags?: string | number }> = []
  const handles = new Map<number, number>()
  let nextHandle = 1
  vi.stubGlobal('__desktop', {
    invoke: (
      channel: string,
      options: {
        path: string
        flags?: string | number
        mode?: number
        id: number
      }
    ) => {
      if (channel === 'desktop.io.fs-open') {
        openings.push({ path: options.path, flags: options.flags })
        try {
          const handle = openSync(
            options.path,
            options.flags ?? 'wx',
            options.mode
          )
          const id = nextHandle++
          handles.set(id, handle)
          return Promise.resolve({ id })
        } catch (error) {
          return Promise.reject(error)
        }
      }
      if (channel === 'desktop.io.fs-handle-close') {
        const handle = handles.get(options.id)
        if (handle === undefined) {
          return Promise.reject(new Error('EBADF: bad file descriptor'))
        }
        handles.delete(options.id)
        closeSync(handle)
        return Promise.resolve(null)
      }
      return Promise.reject(new Error(`unexpected ${channel}`))
    },
    invokeSync: () => {
      throw new Error('unexpected synchronous call')
    },
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  })
  return openings
}

describe('LFS progress file creation through the host filesystem', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('creates unique empty files exclusively in the host temporary directory', async () => {
    const openings = installBridge()
    const first = await createLFSProgressFile()
    const second = await createLFSProgressFile()
    try {
      expect(first).not.toBe(second)
      expect(first.startsWith(tmpdir())).toBe(true)
      expect(existsSync(first)).toBe(true)
      expect(existsSync(second)).toBe(true)
      expect(openings).toEqual([
        { path: first, flags: 'wx' },
        { path: second, flags: 'wx' },
      ])
    } finally {
      rmSync(first, { force: true })
      rmSync(second, { force: true })
    }
  })
})
