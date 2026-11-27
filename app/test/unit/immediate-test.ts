import { afterEach, describe, it, vi } from 'vitest'
import assert from 'node:assert'

import { cancelImmediate, scheduleImmediate } from '../../src/lib/immediate'

const flush = () => new Promise(resolve => setTimeout(resolve, 10))

describe('scheduleImmediate', () => {
  it('runs callbacks with their arguments', async () => {
    const seen: Array<string | number> = []
    scheduleImmediate(
      (first: string, second: number) => seen.push(first, second),
      'one',
      2
    )

    await flush()
    assert.deepStrictEqual(seen, ['one', 2])
  })

  it('runs callbacks in scheduling order', async () => {
    const seen: number[] = []
    scheduleImmediate(() => seen.push(1))
    scheduleImmediate(() => seen.push(2))
    scheduleImmediate(() => seen.push(3))

    await flush()
    assert.deepStrictEqual(seen, [1, 2, 3])
  })

  it('continues scheduling after the queue drains', async () => {
    const seen: number[] = []
    scheduleImmediate(() => seen.push(1))
    await flush()

    scheduleImmediate(() => seen.push(2))
    await flush()
    assert.deepStrictEqual(seen, [1, 2])
  })

  it('preserves work scheduled from inside an immediate', async () => {
    const seen: number[] = []
    scheduleImmediate(() => {
      seen.push(1)
      scheduleImmediate(() => seen.push(2))
    })

    await flush()
    assert.deepStrictEqual(seen, [1, 2])
  })

  it('cancels only the requested callback', async () => {
    const seen: number[] = []
    const cancelled = scheduleImmediate(() => seen.push(1))
    scheduleImmediate(() => seen.push(2))
    cancelImmediate(cancelled)

    await flush()
    assert.deepStrictEqual(seen, [2])
  })

  it('cancels a pending callback', async () => {
    let ran = false
    const immediate = scheduleImmediate(() => {
      ran = true
    })
    cancelImmediate(immediate)

    await flush()
    assert.strictEqual(ran, false)
  })
})

describe('install-immediate', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('installs only the narrow global surface dependencies expect', async () => {
    vi.stubGlobal('setImmediate', undefined)
    vi.stubGlobal('clearImmediate', undefined)
    vi.resetModules()
    await import('../../src/lib/install-immediate')

    const seen: string[] = []
    globalThis.setImmediate((value: string) => seen.push(value), 'ran')
    const cancelled = globalThis.setImmediate(() => seen.push('cancelled'))
    globalThis.clearImmediate(cancelled)

    await flush()
    assert.deepStrictEqual(seen, ['ran'])
  })
})
