import assert from 'node:assert/strict'
import { afterEach, beforeEach, describe, it, vi } from 'vitest'

const invokeElectron = vi.hoisted(() => vi.fn())
vi.mock('../../src/lib/ipc-renderer', () => ({ invoke: invokeElectron }))

import { openPath } from '../../src/lib/open-path'

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})
beforeEach(() => invokeElectron.mockReset())

function simulateElectron() {
  const get = Reflect.get
  vi.spyOn(Reflect, 'get').mockImplementation((target, key, receiver) =>
    target === window && key === 'process'
      ? { versions: { electron: '44.0.0' } }
      : get(target, key, receiver)
  )
}

describe('web renderer openPath', () => {
  it('passes the original absolute path to the Wry shell', async () => {
    const invoke = vi.fn(async () => '')
    vi.stubGlobal('__desktop', { platform: 'wry', invoke })
    const path = "/tmp/-资料 '$?.txt"
    assert.equal(await openPath(path), '')
    assert.deepEqual(invoke.mock.calls, [['desktop.io.open-path', { path }]])
  })

  it('returns the native launcher error without hiding it', async () => {
    const invoke = vi.fn(
      async () => 'The default application could not open the path.'
    )
    vi.stubGlobal('__desktop', { platform: 'wry', invoke })
    assert.equal(
      await openPath('/tmp/file'),
      'The default application could not open the path.'
    )
  })

  it('does not ask a WebSocket host to open a server-side file', async () => {
    const invoke = vi.fn()
    vi.stubGlobal('__desktop', { __transport: 'ws', invoke })
    assert.notEqual(await openPath('/tmp/file'), '')
    assert.equal(invoke.mock.calls.length, 0)
  })

  it('reports an uncertain transport outcome without retrying', async () => {
    const invoke = vi.fn(async () => {
      throw new Error('disconnected')
    })
    vi.stubGlobal('__desktop', { platform: 'wry', invoke })
    assert.notEqual(await openPath('/tmp/file'), '')
    assert.equal(invoke.mock.calls.length, 1)
  })

  it('uses the typed native channel for the Electron host', async () => {
    vi.stubGlobal('__desktop', undefined)
    simulateElectron()
    invokeElectron.mockResolvedValue('')
    const path = "/tmp/-资料 '$?.txt"
    assert.equal(await openPath(path), '')
    assert.deepEqual(invokeElectron.mock.calls, [
      ['desktop.io.open-path', { path }],
    ])
  })

  it('preserves the Electron host error', async () => {
    vi.stubGlobal('__desktop', undefined)
    simulateElectron()
    invokeElectron.mockResolvedValue('open failed')
    assert.equal(await openPath('/tmp/file'), 'open failed')
    assert.equal(invokeElectron.mock.calls.length, 1)
  })

  it('does not invoke a host when a standalone browser has no bridge', async () => {
    vi.stubGlobal('__desktop', undefined)
    assert.notEqual(await openPath('/tmp/file'), '')
    assert.equal(invokeElectron.mock.calls.length, 0)
  })

  it('reports an invalid Electron reply without retrying', async () => {
    vi.stubGlobal('__desktop', undefined)
    simulateElectron()
    invokeElectron.mockResolvedValue(null)
    assert.notEqual(await openPath('/tmp/file'), '')
    assert.equal(invokeElectron.mock.calls.length, 1)
  })

  it('preserves an Electron transport exception without retrying', async () => {
    vi.stubGlobal('__desktop', undefined)
    simulateElectron()
    const error = new Error('IPC failed')
    invokeElectron.mockRejectedValue(error)
    await assert.rejects(openPath('/tmp/file'), error)
    assert.equal(invokeElectron.mock.calls.length, 1)
  })
})
