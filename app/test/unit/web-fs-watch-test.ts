import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import {
  appendFileSync,
  mkdirSync,
  mkdtempSync,
  rmdirSync,
  rmSync,
  statSync,
  existsSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { rmdir, watch } from '../../src/web/fs'

// `watch` and `rmdir` against a bridge answering `fs-stat` and `fs-rmdir` from
// the real file system, the way the Rust host does.

function installBridge() {
  const invoke = (channel: string, options: { path: string }) => {
    if (channel === 'desktop.io.fs-stat') {
      try {
        const meta = statSync(options.path, { bigint: false })
        return Promise.resolve({
          mode: meta.mode,
          size: meta.size,
          dev: meta.dev,
          ino: String(meta.ino),
          mtimeMs: meta.mtimeMs,
          atimeMs: meta.atimeMs,
          ctimeMs: meta.ctimeMs,
          birthtimeMs: meta.birthtimeMs,
          isFile: meta.isFile(),
          isDirectory: meta.isDirectory(),
        })
      } catch (error) {
        return Promise.reject(Object.assign(error as Error, { code: 'ENOENT' }))
      }
    }
    if (channel === 'desktop.io.fs-rmdir') {
      rmdirSync(options.path)
      return Promise.resolve(undefined)
    }
    return Promise.reject(new Error(`unexpected ${channel}`))
  }
  ;(globalThis as any).__desktop = {
    invoke,
    invokeSync: () => {
      throw new Error('unexpected synchronous call')
    },
  }
}

describe('fs adapter watch and rmdir', () => {
  let directory: string

  beforeEach(() => {
    directory = mkdtempSync(join(tmpdir(), 'desktop-web-fs-watch-'))
    installBridge()
  })

  afterEach(() => {
    delete (globalThis as any).__desktop
    rmSync(directory, { recursive: true, force: true })
  })

  it('reports a change when the watched file grows and stops on close', async () => {
    const file = join(directory, 'progress')
    writeFileSync(file, 'first\n')
    const events: string[] = []
    const watcher = watch(file, (event: string) => events.push(event))
    await new Promise(resolve => setTimeout(resolve, 400))
    expect(events).toEqual([])

    appendFileSync(file, 'second\n')
    await new Promise(resolve => setTimeout(resolve, 600))
    expect(events).toEqual(['change'])

    watcher.close()
    appendFileSync(file, 'third\n')
    await new Promise(resolve => setTimeout(resolve, 600))
    expect(events).toEqual(['change'])
  })

  it('reports an error and stops when the watched file disappears', async () => {
    const file = join(directory, 'gone')
    writeFileSync(file, 'x')
    const watcher = watch(file, () => {})
    const errors: unknown[] = []
    watcher.on('error', error => errors.push(error))
    await new Promise(resolve => setTimeout(resolve, 300))
    rmSync(file)
    await new Promise(resolve => setTimeout(resolve, 600))
    expect(errors).toHaveLength(1)
  })

  it('removes an empty directory through the callback form', async () => {
    const target = join(directory, 'empty')
    mkdirSync(target)
    await new Promise<void>((resolve, reject) =>
      rmdir(target, (error: Error | null) =>
        error ? reject(error) : resolve()
      )
    )
    expect(existsSync(target)).toBe(false)
  })
})
