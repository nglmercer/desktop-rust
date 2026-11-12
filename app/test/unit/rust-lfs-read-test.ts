import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  available: vi.fn(),
  usesLfs: vi.fn(),
  tracking: vi.fn(),
  installHooks: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    lfsAvailable: mocks.available,
    repositoryUsesLfs: mocks.usesLfs,
    repositoryLfsFileTracking: mocks.tracking,
    installRepositoryLfsHooks: mocks.installHooks,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))

const repository = { path: '/repo' } as Repository

describe('Rust-backed Git LFS reads', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.clearAllMocks()
    mocks.transport = 'ws'
  })

  it('uses scoped status and attributes without invoking the Git shim', async () => {
    mocks.available.mockResolvedValue(true)
    mocks.usesLfs.mockResolvedValue(true)
    mocks.tracking.mockResolvedValue([true])
    const { isUsingLFS, isTrackedByLFS } = await import('../../src/lib/git/lfs')
    await expect(isUsingLFS(repository)).resolves.toBe(true)
    await expect(isTrackedByLFS(repository, '-file.bin')).resolves.toBe(true)
    expect(mocks.usesLfs).toHaveBeenCalledWith('/repo')
    expect(mocks.tracking).toHaveBeenCalledWith('/repo', ['-file.bin'])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('batches file checks while preserving order and duplicates', async () => {
    const paths = Array.from({ length: 1001 }, (_, index) =>
      index % 2 ? 'plain' : 'large'
    )
    mocks.tracking.mockImplementation(
      async (_repository: string, batch: string[]) =>
        batch.map(path => path === 'large')
    )
    const { filesNotTrackedByLFS } = await import('../../src/lib/git/lfs')
    await expect(filesNotTrackedByLFS(repository, paths)).resolves.toEqual(
      paths.filter(path => path === 'plain')
    )
    expect(mocks.tracking.mock.calls.map(call => call[1].length)).toEqual([
      1000, 1,
    ])
  })

  it('installs hooks through the scoped backend with explicit force', async () => {
    mocks.available.mockResolvedValue(true)
    mocks.installHooks.mockResolvedValue(undefined)
    const { installLFSHooks } = await import('../../src/lib/git/lfs')
    await installLFSHooks(repository, false)
    await installLFSHooks(repository, true)
    expect(mocks.installHooks.mock.calls).toEqual([
      ['/repo', false],
      ['/repo', true],
    ])
    expect(mocks.git).not.toHaveBeenCalled()
    mocks.installHooks.mockRejectedValueOnce(new Error('hook conflict'))
    await expect(installLFSHooks(repository, false)).rejects.toThrow(
      'hook conflict'
    )
  })

  it('retries availability after a backend failure instead of caching it as missing', async () => {
    mocks.available
      .mockRejectedValueOnce(new Error('disconnected'))
      .mockResolvedValueOnce(true)
    const { isLFSAvailable } = await import('../../src/lib/git/lfs')
    await expect(isLFSAvailable()).rejects.toThrow('disconnected')
    await expect(isLFSAvailable()).resolves.toBe(true)
    expect(mocks.available).toHaveBeenCalledTimes(2)
  })
})
