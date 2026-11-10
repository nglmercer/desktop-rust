import { describe, it, beforeEach, afterEach } from 'node:test'
import assert from 'node:assert'
import { mkdtempSync, openSync as nodeOpen, readSync as nodeRead, closeSync as nodeClose, writeFileSync, rmSync, renameSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createReadStream, openSync, readSync, closeSync } from '../../src/web/fs'

// The `fs` adapter against a bridge that answers `fs-read-range` from the real
// file system, the way Rust does. What is under test is the adapter's block
// chaining, inclusive `end`, encodings, EOF and the replaced-file check.

const BLOCK = 64 * 1024

function installRangeBridge() {
  const answer = (o: { path: string; offset: number; length: number }) => {
    const fd = nodeOpen(o.path, 'r')
    try {
      const meta = statSync(o.path)
      const buffer = Buffer.alloc(o.length)
      const bytesRead = nodeRead(fd, buffer, 0, o.length, o.offset)
      return {
        data: buffer.subarray(0, bytesRead).toString('base64'),
        bytesRead,
        eof: o.offset + bytesRead >= meta.size,
        identity: { dev: meta.dev, ino: String(meta.ino), size: meta.size, mtimeMs: meta.mtimeMs },
      }
    } finally {
      nodeClose(fd)
    }
  }
  const bridge = {
    invoke: (channel: string, options: any) =>
      channel === 'desktop.io.fs-read-range'
        ? Promise.resolve(answer(options))
        : Promise.reject(new Error(`unexpected ${channel}`)),
    invokeSync: (channel: string, options: any) => {
      if (channel === 'desktop.io.fs-stat') {
        // `openSync` only needs the path to exist.
        const meta = statSync(options.path)
        return { isFile: meta.isFile(), isDirectory: meta.isDirectory(), size: meta.size }
      }
      if (channel !== 'desktop.io.fs-read-range') {
        throw new Error(`unexpected ${channel}`)
      }
      return answer(options)
    },
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  }
  ;(globalThis as any).__desktop = bridge
}

/** Put `data` at `path` through a new file renamed over it, as editors do. */
function replaceFile(path: string, data: Buffer) {
  const staged = `${path}.staged`
  writeFileSync(staged, data)
  renameSync(staged, path)
}

function collect(stream: ReturnType<typeof createReadStream>) {
  return new Promise<Buffer[]>((resolve, reject) => {
    const chunks: Buffer[] = []
    stream.on('data', (chunk: Buffer | string) => {
      chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : chunk)
    })
    stream.on('error', reject)
    stream.on('end', () => resolve(chunks))
  })
}

describe('fs adapter ranges', () => {
  let dir: string
  let file: string
  let content: Buffer

  beforeEach(() => {
    installRangeBridge()
    dir = mkdtempSync(join(tmpdir(), 'desktop-fs-range-'))
    file = join(dir, 'data.bin')
    // Larger than two blocks, so reads must be chained.
    content = Buffer.from(Array.from({ length: 2 * BLOCK + 300 }, (_, i) => i % 251))
    writeFileSync(file, content)
  })

  afterEach(() => {
    delete (globalThis as any).__desktop
    rmSync(dir, { recursive: true, force: true })
  })

  it('streams a whole file across blocks, byte for byte', async () => {
    const chunks = await collect(createReadStream(file))
    assert.deepStrictEqual(Buffer.concat(chunks), content)
    assert.ok(chunks.length >= 3, 'a file larger than a block arrives in several chunks')
  })

  it('treats start and end as Node does, with an inclusive end', async () => {
    const chunks = await collect(createReadStream(file, { start: 10, end: 19 }))
    assert.deepStrictEqual(Buffer.concat(chunks), content.subarray(10, 20))
  })

  it('decodes text whose characters straddle a block boundary', async () => {
    // "é" straddles the first boundary: the first byte ends one block, the second starts the next.
    const text = Buffer.concat([Buffer.alloc(BLOCK - 1, 0x61), Buffer.from('é', 'utf8'), Buffer.from('z')])
    writeFileSync(file, text)
    const chunks = await collect(createReadStream(file, { encoding: 'utf8' }))
    assert.strictEqual(chunks.map(c => c.toString()).join(''), text.toString('utf8'))
  })

  it('fails explicitly when the file is replaced between blocks', async () => {
    const bridge = (globalThis as any).__desktop
    const invoke = bridge.invoke
    let calls = 0
    bridge.invoke = (channel: string, options: any) => {
      calls += 1
      if (calls === 2) {
        // Replace the file after the first block was read.
        replaceFile(file, Buffer.alloc(content.length, 0x7a))
      }
      return invoke(channel, options)
    }
    await assert.rejects(collect(createReadStream(file)), (error: any) => {
      assert.strictEqual(error.code, 'EIO')
      return true
    })
  })

  it('reads a descriptor by cursor and never mixes files across calls', () => {
    const fd = openSync(file)
    const first = Buffer.alloc(8)
    assert.strictEqual(readSync(fd, first, 0, 8, null), 8)
    assert.deepStrictEqual(first, content.subarray(0, 8))
    const second = Buffer.alloc(8)
    assert.strictEqual(readSync(fd, second, 0, 8, null), 8, 'the cursor advances')
    assert.deepStrictEqual(second, content.subarray(8, 16))

    // Replacing the file under an open descriptor is an explicit failure.
    replaceFile(file, Buffer.alloc(content.length, 0x01))
    assert.throws(() => readSync(fd, Buffer.alloc(8), 0, 8, 0), (error: any) => error.code === 'EIO')
    closeSync(fd)
  })

  it('reads a range without transferring the whole file', () => {
    const transferred: number[] = []
    const bridge = (globalThis as any).__desktop
    // The synchronous path is the thing under test, so its spy names it.
    // eslint-disable-next-line no-sync
    const invokeSync = bridge.invokeSync
    // eslint-disable-next-line no-sync
    bridge.invokeSync = (channel: string, options: any) => {
      if (channel === 'desktop.io.fs-read-range') {
        transferred.push(options.length)
      }
      return invokeSync(channel, options)
    }
    const fd = openSync(file)
    const buffer = Buffer.alloc(BLOCK + 10)
    const read = readSync(fd, buffer, 0, buffer.length, BLOCK - 5)
    closeSync(fd)
    assert.strictEqual(read, buffer.length)
    assert.deepStrictEqual(buffer.subarray(0, read), content.subarray(BLOCK - 5, BLOCK - 5 + read))
    assert.ok(transferred.every(length => length <= BLOCK), 'no call asks for more than one block')
    assert.ok(statSync(file).size > transferred.reduce((a, b) => a + b, 0), 'the file was not read whole')
  })
})
