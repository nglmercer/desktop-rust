import assert from 'node:assert'
import Fs from 'node:fs'
import { appendFile, stat, writeFile } from 'node:fs/promises'
import { once } from 'node:events'
import { join } from 'node:path'
import { beforeEach, describe, it, mock, TestContext } from 'node:test'
import { vi } from 'vitest'

import { tailByLine } from '../../src/lib/file-system'
import { Tailer } from '../../src/lib/tailer'
import { createTempDirectory } from '../helpers/temp'

// The tailer calls `watch` through the `node:fs` namespace, which cannot be
// patched in place under Vitest. Route it through a spy that calls the real
// watcher, and reset it per test.
const realWatch = Fs.watch
const watchSpy = mock.fn((...args: Parameters<typeof Fs.watch>) =>
  (realWatch as (...a: Array<unknown>) => unknown)(...args)
)
const statSpy = mock.fn((...args: Array<unknown>) =>
  (Fs.stat as unknown as (...a: Array<unknown>) => unknown)(...args)
)
const createReadStreamSpy = mock.fn((...args: Array<unknown>) =>
  (Fs.createReadStream as unknown as (...a: Array<unknown>) => unknown)(...args)
)
vi.mock('node:fs', async importOriginal => ({
  ...(await importOriginal<typeof import('node:fs')>()),
  watch: (...args: Parameters<typeof Fs.watch>) => watchSpy(...args),
  stat: (...args: Array<unknown>) => (statSpy as any)(...args),
  createReadStream: (...args: Array<unknown>) =>
    (createReadStreamSpy as any)(...args),
}))

async function setupTailer(t: TestContext) {
  const directory = await createTempDirectory(t)
  const path = join(directory, 'progress')
  await writeFile(path, 'first\n')

  const watch = (watchSpy.mock.resetCalls(), watchSpy)
  const tailer = new Tailer(path)
  t.after(() => tailer.stop())
  tailer.start()

  const watcher = watch.mock.calls[0].result
  assert.ok(watcher)
  // Drive watch events explicitly instead of relying on platform timing.
  watcher.close()
  const close = t.mock.method(watcher, 'close')
  const stats = await stat(path)
  const pendingStats: Array<
    (error: NodeJS.ErrnoException | null, stats: Fs.Stats) => void
  > = []
  statSpy.mock.resetCalls()
  statSpy.mock.mockImplementation(
    (
      _path: Fs.PathLike,
      callback: (error: NodeJS.ErrnoException | null, stats: Fs.Stats) => void
    ) => {
      pendingStats.push(callback)
    }
  )
  createReadStreamSpy.mock.resetCalls()
  const read = createReadStreamSpy
  const completeStat = (size = stats.size, error: Error | null = null) => {
    const callback = pendingStats.shift()
    assert.ok(callback, 'Expected a pending stat')
    stats.size = size
    callback(error, stats)
  }

  const nextStream = () =>
    new Promise<Fs.ReadStream>(resolve => {
      const subscription = tailer.onDataAvailable(stream => {
        subscription.dispose()
        t.after(() => stream.destroy())
        resolve(stream)
      })
      t.after(() => subscription.dispose())
    })

  return {
    tailer,
    watcher,
    close,
    nextStream,
    path,
    watch,
    read,
    pendingStats,
    completeStat,
  }
}

async function consume(stream: Fs.ReadStream) {
  const closed = once(stream, 'close')
  const chunks: Buffer[] = []
  stream.on('data', (chunk: Buffer) => chunks.push(chunk))
  await closed
  return Buffer.concat(chunks).toString()
}

// Each test starts from the real file-system functions; a test that replaces
// them does so for its own duration only.
beforeEach(() => {
  statSpy.mock.mockImplementation((...args: Array<unknown>) =>
    (Fs.stat as unknown as (...a: Array<unknown>) => unknown)(...args)
  )
  createReadStreamSpy.mock.mockImplementation((...args: Array<unknown>) =>
    (Fs.createReadStream as unknown as (...a: Array<unknown>) => unknown)(
      ...args
    )
  )
})

describe('Tailer', { timeout: 5000 }, () => {
  it('reports read stream errors and stops watching', async t => {
    const { tailer, watcher, close, nextStream, completeStat } =
      await setupTailer(t)
    const onError = t.mock.fn()
    tailer.onError(onError)
    const next = nextStream()
    watcher.emit('change', 'change')
    completeStat()
    const stream = await next
    const error = Object.assign(new Error('Too many open files'), {
      code: 'EMFILE',
    })

    watcher.emit('change', 'change')
    assert.doesNotThrow(() => stream.emit('error', error))
    assert.strictEqual(onError.mock.callCount(), 1)
    assert.strictEqual(onError.mock.calls[0].arguments[0], error)
    assert.strictEqual(close.mock.callCount(), 1)
  })

  it('coalesces changes into one stat and one open stream at a time', async t => {
    const { watcher, nextStream, path, read, pendingStats, completeStat } =
      await setupTailer(t)
    const first = nextStream()

    for (let i = 0; i < 100; i++) {
      watcher.emit('change', 'change')
    }

    assert.strictEqual(pendingStats.length, 1)
    completeStat()
    const firstStream = await first
    await appendFile(path, 'second\n')

    for (let i = 0; i < 100; i++) {
      watcher.emit('change', 'change')
    }

    assert.strictEqual(pendingStats.length, 0)
    assert.strictEqual(read.mock.callCount(), 1)
    assert.strictEqual(await consume(firstStream), 'first\n')
    assert.strictEqual(pendingStats.length, 1)

    const second = nextStream()
    completeStat(13)
    assert.strictEqual(await consume(await second), 'second\n')
    assert.strictEqual(read.mock.callCount(), 2)
    assert.strictEqual(pendingStats.length, 0)
  })

  it('retains changes received during a stat that finds no growth', async t => {
    const { watcher, nextStream, pendingStats, completeStat } =
      await setupTailer(t)
    const next = nextStream()
    watcher.emit('change', 'change')
    watcher.emit('change', 'change')
    completeStat(0)

    assert.strictEqual(pendingStats.length, 1)
    completeStat(6)
    assert.strictEqual(await consume(await next), 'first\n')
    assert.strictEqual(pendingStats.length, 0)
  })

  it('does not read past the observed file size', async t => {
    const { watcher, nextStream, path, completeStat } = await setupTailer(t)
    const next = nextStream()
    watcher.emit('change', 'change')
    completeStat()
    const stream = await next
    await appendFile(path, 'second\n')

    assert.strictEqual(await consume(stream), 'first\n')
  })

  it('destroys the active stream when stopped', async t => {
    const { tailer, watcher, nextStream, close, completeStat } =
      await setupTailer(t)
    const next = nextStream()
    watcher.emit('change', 'change')
    completeStat()
    const stream = await next
    const closed = once(stream, 'close')

    tailer.stop()
    tailer.stop()

    assert.strictEqual(stream.destroyed, true)
    await closed
    assert.strictEqual(close.mock.callCount(), 1)
  })

  it('reports stat failures and stops watching', async t => {
    const { tailer, watcher, close, read, completeStat } = await setupTailer(t)
    const onError = t.mock.fn()
    tailer.onError(onError)
    const error = Object.assign(new Error('File no longer exists'), {
      code: 'ENOENT',
    })
    watcher.emit('change', 'change')
    completeStat(0, error)

    assert.strictEqual(onError.mock.callCount(), 1)
    assert.strictEqual(onError.mock.calls[0].arguments[0], error)
    assert.strictEqual(close.mock.callCount(), 1)
    assert.strictEqual(read.mock.callCount(), 0)
  })

  it('destroys the active stream on watcher failure', async t => {
    const { tailer, watcher, nextStream, completeStat } = await setupTailer(t)
    const onError = t.mock.fn()
    tailer.onError(onError)
    const next = nextStream()
    watcher.emit('change', 'change')
    completeStat()
    const stream = await next
    const error = new Error('Watcher failed')

    watcher.emit('error', error)

    assert.strictEqual(stream.destroyed, true)
    assert.strictEqual(onError.mock.callCount(), 1)
    assert.strictEqual(onError.mock.calls[0].arguments[0], error)
  })

  it('ignores old stat callbacks and watcher events after restarting', async t => {
    const {
      tailer,
      watcher,
      watch,
      read,
      nextStream,
      pendingStats,
      completeStat,
    } = await setupTailer(t)
    const onError = t.mock.fn()
    tailer.onError(onError)
    watcher.emit('change', 'change')
    tailer.stop()
    tailer.start()
    const newWatcher = watch.mock.calls[1].result
    assert.ok(newWatcher)
    newWatcher.close()
    const close = t.mock.method(newWatcher, 'close')

    completeStat()
    watcher.emit('change', 'change')
    watcher.emit('error', new Error('Old watcher failed'))

    assert.strictEqual(read.mock.callCount(), 0)
    assert.strictEqual(pendingStats.length, 0)
    assert.strictEqual(close.mock.callCount(), 0)
    assert.strictEqual(onError.mock.callCount(), 0)

    const next = nextStream()
    newWatcher.emit('change', 'change')
    completeStat()
    assert.strictEqual(await consume(await next), 'first\n')
  })

  it('ignores late stream errors and close events after restarting', async t => {
    const { tailer, watcher, watch, nextStream, pendingStats, completeStat } =
      await setupTailer(t)
    const onError = t.mock.fn()
    tailer.onError(onError)
    const next = nextStream()
    watcher.emit('change', 'change')
    completeStat()
    const stream = await next
    watcher.emit('change', 'change')
    tailer.stop()
    tailer.start()
    const newWatcher = watch.mock.calls[1].result
    assert.ok(newWatcher)
    newWatcher.close()
    const close = t.mock.method(newWatcher, 'close')

    stream.emit('error', new Error('Old read failed'))
    stream.emit('close')

    assert.strictEqual(pendingStats.length, 0)
    assert.strictEqual(close.mock.callCount(), 0)
    assert.strictEqual(onError.mock.callCount(), 0)
  })

  it('ignores non-change events and files that have not grown', async t => {
    const { watcher, read, nextStream, pendingStats, completeStat } =
      await setupTailer(t)
    watcher.emit('change', 'rename')
    assert.strictEqual(pendingStats.length, 0)
    const next = nextStream()
    watcher.emit('change', 'change')
    completeStat()
    assert.strictEqual(await consume(await next), 'first\n')

    watcher.emit('change', 'change')
    completeStat()
    watcher.emit('change', 'change')
    completeStat(0)

    assert.strictEqual(read.mock.callCount(), 1)
  })
})

describe('tailByLine', { timeout: 5000 }, () => {
  it('logs an asynchronous EMFILE open failure without emitting lines', async t => {
    const directory = await createTempDirectory(t)
    const path = join(directory, 'progress')
    await writeFile(path, 'first\n')
    const error = Object.assign(new Error('Too many open files'), {
      code: 'EMFILE',
      syscall: 'open',
      path,
    })
    const createReadStream = Fs.createReadStream
    const failOpen: typeof Fs.createReadStream = (path, options) =>
      createReadStream(path, {
        ...(typeof options === 'string' ? { encoding: options } : options),
        fs: {
          open: (_path, _flags, _mode, callback) => {
            queueMicrotask(() => callback(error, 0))
          },
          read: Fs.read,
          close: Fs.close,
        },
      })
    createReadStreamSpy.mock.mockImplementation(failOpen)
    const warning = new Promise<Error | undefined>(resolve => {
      t.mock.method(log, 'warn', (_message: string, error?: Error) => {
        resolve(error)
      })
    })
    const watch = (watchSpy.mock.resetCalls(), watchSpy)
    const onLine = t.mock.fn()
    const disposable = tailByLine(path, onLine)
    t.after(() => disposable.dispose())
    const watcher = watch.mock.calls[0].result
    assert.ok(watcher)
    watcher.close()
    watcher.emit('change', 'change')

    assert.strictEqual(await warning, error)
    assert.strictEqual(onLine.mock.callCount(), 0)
  })

  it('continues reporting lines across file updates', async t => {
    const directory = await createTempDirectory(t)
    const path = join(directory, 'progress')
    await writeFile(path, '')
    const lines: string[] = []
    let onLine: (line: string) => void = () => {}
    const nextLine = () =>
      new Promise<string>(resolve => {
        onLine = resolve
      })
    const disposable = tailByLine(path, line => {
      lines.push(line)
      onLine(line)
    })
    t.after(() => disposable.dispose())

    const first = nextLine()
    await appendFile(path, 'first\n')
    assert.strictEqual(await first, 'first')
    const second = nextLine()
    await appendFile(path, 'second\n')
    assert.strictEqual(await second, 'second')
    assert.deepStrictEqual(lines, ['first', 'second'])
  })
})
