import { describe, it, beforeEach } from 'vitest'
import assert from 'node:assert'
import { once } from 'node:events'
import { spawn } from '../../src/web/child_process'

// The webview adapter, driven against a fake of the Rust side. The fake holds
// one stdin block at a time and refuses the next with EBUSY until the test
// releases it and reports `drain`, as the Rust writer does. It also records
// acknowledgements, so flow control is observable.

const LIMITS = { block: 8, inputBudget: 64, outputWindow: 32 }

interface IFakeRust {
  readonly accepted: Uint8Array[]
  readonly calls: Array<{ channel: string; options: any }>
  readonly acks: Array<{ stream: string; seq: number }>
  refusals: number
  /** Clear the record between tests. The adapter keeps one router per process. */
  reset(): void
  /** The child reads the held block; a refused writer is then told to resume. */
  release(): void
  emit(message: Record<string, unknown>): void
}

function installFakeRust(): IFakeRust {
  const listeners = new Map<string, (event: unknown, message: any) => void>()
  const accepted: Uint8Array[] = []
  const calls: Array<{ channel: string; options: any }> = []
  const acks: Array<{ stream: string; seq: number }> = []
  let held: Uint8Array | null = null
  let wantsDrain = false
  let nextId = 1
  // The id of the child most recently spawned; Rust tags every event with it.
  let lastId = 0
  const fake: IFakeRust = {
    accepted,
    calls,
    acks,
    refusals: 0,
    reset() {
      accepted.length = 0
      calls.length = 0
      acks.length = 0
      fake.refusals = 0
      held = null
      wantsDrain = false
      nextId = 1
      lastId = 0
    },
    release() {
      held = null
      if (wantsDrain) {
        wantsDrain = false
        fake.emit({ id: lastId, event: 'drain' })
      }
    },
    emit(message) {
      listeners.get('desktop-process')?.(null, message)
    },
  }
  const bridge = {
    invoke(channel: string, options: any): Promise<any> {
      calls.push({ channel, options })
      switch (channel) {
        case 'desktop.io.process-spawn': {
          const id = nextId++
          lastId = id
          return Promise.resolve({ id, pid: 1000 + id, limits: LIMITS })
        }
        case 'desktop.io.process-write': {
          const data = new Uint8Array(Buffer.from(options.data, 'base64'))
          if (held !== null) {
            fake.refusals += 1
            wantsDrain = true
            return Promise.reject(Object.assign(new Error('EBUSY: QUEUE_FULL'), { code: 'EBUSY' }))
          }
          held = data
          accepted.push(data)
          return Promise.resolve(null)
        }
        default:
          return Promise.resolve(null)
      }
    },
    send(channel: string, options: any) {
      calls.push({ channel, options })
      if (channel === 'desktop.io.process-ack') {
        acks.push({ stream: options.stream, seq: options.seq })
      }
    },
    on(channel: string, listener: (event: unknown, message: any) => void) {
      listeners.set(channel, listener)
      return bridge
    },
    encodeBase64: (bytes: Uint8Array) => Buffer.from(bytes).toString('base64'),
    decodeBase64: (text: string) => new Uint8Array(Buffer.from(text, 'base64')),
  }
  ;(globalThis as any).__desktop = bridge
  return fake
}

// One bridge for the whole file: the adapter installs its event router once.
const fake = installFakeRust()

const settle = () => new Promise(resolve => setImmediate(resolve))

describe('child_process stdin flow control', () => {
  beforeEach(() => {
    fake.reset()
  })

  it('returns false above a block, keeps every byte in order, and drains', async () => {
    const child = spawn('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] })
    await once(child, 'spawn')

    const payload = Uint8Array.from({ length: 30 }, (_, i) => i)
    const drained = once(child.stdin!, 'drain')
    assert.strictEqual(child.stdin!.write(payload), false, 'a backlog reports false')

    // Release the child's reads one block at a time until the queue is empty.
    for (let step = 0; step < 50; step++) {
      await settle()
      fake.release()
    }
    await drained

    const sent = fake.accepted.flatMap(block => [...block])
    assert.deepStrictEqual(sent, [...payload], 'the bytes arrive intact and in order')
    assert.ok(fake.accepted.every(block => block.length <= LIMITS.block), 'blocks respect the block size')
    assert.ok(fake.refusals > 0, 'Rust refused a block while it was still held, and the writer waited')
  })

  it('a write callback runs once per chunk, after its last block is accepted', async () => {
    const child = spawn('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] })
    await once(child, 'spawn')

    let callbacks = 0
    child.stdin!.write(new Uint8Array(20), () => {
      callbacks += 1
    })
    for (let step = 0; step < 50; step++) {
      await settle()
      fake.release()
    }
    await settle()
    assert.strictEqual(callbacks, 1)
  })

  it('fails explicitly and stops the child when a producer ignores false past the budget', async () => {
    const child = spawn('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] })
    await once(child, 'spawn')

    const failed = once(child.stdin!, 'error')
    child.stdin!.on('error', () => {})
    child.stdin!.write(new Uint8Array(LIMITS.inputBudget + 1))

    const [error] = await failed
    assert.strictEqual((error as any).code, 'EOVERFLOW')
    await settle()
    assert.ok(
      fake.calls.some(call => call.channel === 'desktop.io.process-kill'),
      'the child is stopped rather than left running'
    )
  })

  it('ends input only after every accepted write, and reports finish', async () => {
    const child = spawn('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] })
    await once(child, 'spawn')

    const finished = once(child.stdin!, 'finish')
    child.stdin!.write(new Uint8Array([1, 2, 3]))
    child.stdin!.end()
    for (let step = 0; step < 20; step++) {
      await settle()
      fake.release()
    }
    await finished

    const channels = fake.calls.map(call => call.channel)
    const lastWrite = channels.lastIndexOf('desktop.io.process-write')
    const end = channels.indexOf('desktop.io.process-end')
    assert.ok(end > lastWrite, 'end-of-input is sent after the last write')
    assert.deepStrictEqual(fake.accepted.flatMap(block => [...block]), [1, 2, 3])
  })
})

describe('child_process output', () => {
  beforeEach(() => {
    fake.reset()
  })

  it('acknowledges each block only after it reached the consumer', async () => {
    const child = spawn('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] })
    await once(child, 'spawn')

    const seen: Array<{ text: string; acksSoFar: number }> = []
    child.stdout!.setEncoding('utf8')
    child.stdout!.on('data', (text: string) => {
      seen.push({ text, acksSoFar: fake.acks.length })
    })

    fake.emit({ id: 1, event: 'data', stream: 'stdout', seq: 1, data: Buffer.from('ab').toString('base64') })
    fake.emit({ id: 1, event: 'data', stream: 'stdout', seq: 2, data: Buffer.from('cd').toString('base64') })

    assert.deepStrictEqual(seen.map(s => s.text), ['ab', 'cd'])
    assert.deepStrictEqual(seen.map(s => s.acksSoFar), [0, 1], 'an ack follows the consumer')
    assert.deepStrictEqual(fake.acks, [
      { stream: 'stdout', seq: 1 },
      { stream: 'stdout', seq: 2 },
    ])
  })

  it('decodes a multi-byte character split across two blocks', async () => {
    const child = spawn('cat', [], { stdio: ['pipe', 'pipe', 'pipe'] })
    await once(child, 'spawn')

    const text: string[] = []
    child.stdout!.setEncoding('utf8')
    child.stdout!.on('data', (chunk: string) => text.push(chunk))

    // "é" is C3 A9 in UTF-8; the blocks split it.
    fake.emit({ id: 1, event: 'data', stream: 'stdout', seq: 1, data: Buffer.from([0xc3]).toString('base64') })
    fake.emit({ id: 1, event: 'data', stream: 'stdout', seq: 2, data: Buffer.from([0xa9]).toString('base64') })

    assert.strictEqual(text.join(''), 'é')
  })
})
