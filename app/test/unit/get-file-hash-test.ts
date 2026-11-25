import { beforeEach, describe, expect, it, vi } from 'vitest'
import { getFileHash } from '../../src/lib/get-file-hash'

describe('get-file-hash', () => {
  const invoke = vi.fn()

  beforeEach(() => {
    invoke.mockReset()
    ;(globalThis as any).__desktop = { platform: 'wry', invoke }
  })

  it('delegates sha256 and sha1 hashing to the Rust shell', async () => {
    invoke
      .mockResolvedValueOnce('a'.repeat(64))
      .mockResolvedValueOnce('b'.repeat(40))
    await expect(getFileHash('/key.pub', 'sha256')).resolves.toBe(
      'a'.repeat(64)
    )
    await expect(getFileHash('/key.pub', 'sha1')).resolves.toBe('b'.repeat(40))
    expect(invoke.mock.calls).toEqual([
      ['desktop.io.fs-hash', { path: '/key.pub', algorithm: 'sha256' }],
      ['desktop.io.fs-hash', { path: '/key.pub', algorithm: 'sha1' }],
    ])
  })

  it('propagates host filesystem failures', async () => {
    invoke.mockRejectedValueOnce(new Error('missing'))
    await expect(getFileHash('/missing', 'sha256')).rejects.toThrow('missing')
  })
})
