import assert from 'node:assert/strict'
import { afterEach, describe, it, vi } from 'vitest'

import { openPath } from '../../src/lib/open-path'

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

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

  it('does not invoke a host when a standalone browser has no bridge', async () => {
    vi.stubGlobal('__desktop', undefined)
    assert.notEqual(await openPath('/tmp/file'), '')
  })
})
