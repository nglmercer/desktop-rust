import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  canonical: vi.fn(),
  realpath: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    canonicalMutationPath: mocks.canonical,
  }),
}))
vi.mock('fs/promises', () => ({
  realpath: mocks.realpath,
  default: { realpath: mocks.realpath },
}))
vi.mock('../../src/lib/git/core', () => ({ git: vi.fn() }))
import { canonicalMutationPath } from '../../src/lib/git/repository-operation'

describe('canonical mutation destinations', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })
  it.each(['ws', 'wry'])(
    'uses the %s transport without normalizing an absolute symlink path',
    async transport => {
      mocks.transport = transport
      mocks.canonical.mockResolvedValue('/physical/new')
      await expect(canonicalMutationPath('/alias/../new')).resolves.toBe(
        '/physical/new'
      )
      expect(mocks.canonical).toHaveBeenCalledWith('/alias/../new')
      expect(mocks.realpath).not.toHaveBeenCalled()
    }
  )
  it('propagates a bridge failure without guessing a destination', async () => {
    const error = new Error('disconnected')
    mocks.canonical.mockRejectedValue(error)
    await expect(canonicalMutationPath('/repo/new')).rejects.toBe(error)
    expect(mocks.realpath).not.toHaveBeenCalled()
  })
  it('retains the existing native filesystem fallback', async () => {
    mocks.transport = 'none'
    mocks.realpath.mockResolvedValue('/physical/repo')
    await expect(canonicalMutationPath('/alias/repo')).resolves.toBe(
      '/physical/repo'
    )
    expect(mocks.canonical).not.toHaveBeenCalled()
  })
})
