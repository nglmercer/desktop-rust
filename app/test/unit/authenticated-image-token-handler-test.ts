import assert from 'node:assert'
import { afterEach, describe, it, vi } from 'vitest'
import { installAuthenticatedImageTokenHandler } from '../../src/lib/authenticated-image-token-handler'
import { MockIPC } from '../helpers/mock-ipc'

describe('authenticated image token handler', () => {
  afterEach(() => vi.restoreAllMocks())
  it('returns the resolved access token with its request id', async () => {
    const ipc = new MockIPC()
    const resolveToken = vi.fn(async () => 'fresh-token')
    const dispose = installAuthenticatedImageTokenHandler(resolveToken, ipc)
    ipc.emit('resolve-image-token', 12, 'endpoint', 'old-token')
    await Promise.resolve()
    assert.deepStrictEqual(resolveToken.mock.calls[0], [
      'endpoint',
      'old-token',
    ])
    assert.deepStrictEqual(ipc.sends, [
      { channel: 'resolved-image-token', args: [12, 'fresh-token'] },
    ])
    dispose()
  })

  it('sends failure without exposing token-bearing errors', async () => {
    const ipc = new MockIPC()
    const warning = vi.spyOn(log, 'warn')
    const dispose = installAuthenticatedImageTokenHandler(async () => {
      throw new Error('secret refresh token')
    }, ipc)
    ipc.emit('resolve-image-token', 12, 'endpoint', 'old-token')
    await Promise.resolve()
    assert.deepStrictEqual(ipc.sends, [
      { channel: 'resolved-image-token', args: [12, null] },
    ])
    assert.deepStrictEqual(warning.mock.calls[0], [
      'Unable to resolve authentication for a private image',
    ])
    dispose()
  })

  it('removes the listener and drops replies when disposed', async () => {
    const ipc = new MockIPC()
    let finish: (token: string) => void = () => {}
    const resolveToken = vi.fn(
      () =>
        new Promise<string>(resolve => {
          finish = resolve
        })
    )
    const dispose = installAuthenticatedImageTokenHandler(resolveToken, ipc)
    ipc.emit('resolve-image-token', 12, 'endpoint', 'old-token')
    dispose()
    finish('fresh-token')
    await Promise.resolve()
    ipc.emit('resolve-image-token', 13, 'endpoint', 'old-token')
    assert.deepStrictEqual(ipc.sends, [])
    assert.strictEqual(resolveToken.mock.calls.length, 1)
  })
})
