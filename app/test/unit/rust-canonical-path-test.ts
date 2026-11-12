import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'ws',
  canonical: vi.fn(),
  realpath: vi.fn(),
  resources: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    canonicalMutationPath: mocks.canonical,
    repositoryProtectedResources: mocks.resources,
  }),
}))
vi.mock('fs/promises', () => ({
  realpath: mocks.realpath,
  default: { realpath: mocks.realpath },
}))
vi.mock('../../src/lib/git/core', () => ({ git: vi.fn() }))
import {
  canonicalMutationPath,
  hasRepositoryGitOperationAccess,
  withRepositoryGitOperation,
  requireRepositoryGitAsyncOwnership,
  getAssistedCommitProtectedResources,
  getAssistedCommitProtectedPaths,
  protectAssistedCommitResources,
  assertRepositoryGitDestructionAvailable,
} from '../../src/lib/git/repository-operation'

describe('canonical mutation destinations', () => {
  it('retains assisted ownership support when native async hooks are available', () => {
    expect(() => requireRepositoryGitAsyncOwnership()).not.toThrow()
  })
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
  it.each(['ws', 'wry'])(
    'admits repository operations using %s canonicalization',
    async transport => {
      mocks.transport = transport
      mocks.canonical.mockResolvedValueOnce('/physical/repo')
      await withRepositoryGitOperation('/alias/repo', 'mutation', async () => {
        expect(hasRepositoryGitOperationAccess('/alias/repo')).toBe(true)
      })
      expect(mocks.canonical).toHaveBeenCalledWith('/alias/repo')
      expect(mocks.realpath).not.toHaveBeenCalled()
    }
  )
  it('does not enter an operation if the Rust path check fails', async () => {
    mocks.canonical.mockRejectedValueOnce(new Error('disconnected'))
    const operation = vi.fn()
    await expect(
      withRepositoryGitOperation('/repo', 'mutation', operation)
    ).rejects.toThrow('disconnected')
    expect(operation).not.toHaveBeenCalled()
  })
  it.each(['ws', 'wry'])(
    'protects canonical shared object stores from %s discovery',
    async transport => {
      mocks.transport = transport
      mocks.resources.mockResolvedValue({
        paths: [
          '/alias/repo',
          '/repo/.git',
          '/repo/.git',
          '/shared/objects',
          '/physical/repo',
          '/repo/.git',
          '/repo/.git',
          '/physical/shared/objects',
        ],
        objectDirectories: ['/shared/objects'],
        objectRoutingDiagnostics: '',
      })
      const resources = await getAssistedCommitProtectedResources('/alias/repo')
      const protection = protectAssistedCommitResources(
        '/alias/repo',
        resources.paths
      )
      try {
        expect(() =>
          assertRepositoryGitDestructionAvailable('/physical/shared')
        ).toThrow(/assisted commit/)
        expect(mocks.realpath).not.toHaveBeenCalled()
      } finally {
        protection.release()
      }
    }
  )
  it('rejects relative resource paths', async () => {
    mocks.resources.mockResolvedValue({
      paths: [
        '/repo',
        '/repo/.git',
        '/repo/.git',
        'relative',
        '/repo',
        '/repo/.git',
        '/repo/.git',
        'relative',
      ],
      objectDirectories: ['relative'],
      objectRoutingDiagnostics: '',
    })
    await expect(getAssistedCommitProtectedResources('/repo')).rejects.toThrow(
      'Invalid repository resource paths'
    )
  })
  it('rejects unresolved alternate object routing', async () => {
    mocks.resources.mockResolvedValue({
      paths: [
        '/repo',
        '/repo/.git',
        '/repo/.git',
        '/repo/.git/objects',
        '/repo',
        '/repo/.git',
        '/repo/.git',
        '/repo/.git/objects',
      ],
      objectDirectories: ['/repo/.git/objects'],
      objectRoutingDiagnostics: 'unreadable alternate',
    })
    await expect(getAssistedCommitProtectedPaths('/repo')).rejects.toThrow(
      'Cannot verify all Git object stores'
    )
  })
})
