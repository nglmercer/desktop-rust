import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  merged: vi.fn(),
  pointed: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    mergedRepositoryBranches: mocks.merged,
    branchesPointingAt: mocks.pointed,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git/core', () => ({
  git: mocks.git,
  isGitError: () => false,
}))
vi.mock('../../src/lib/git/environment', () => ({
  envForRemoteOperation: vi.fn(),
}))
vi.mock('../../src/lib/git/update-ref', () => ({ deleteRef: vi.fn() }))

import {
  getBranchesPointedAt,
  getMergedBranches,
} from '../../src/lib/git/branch'
import type { Repository } from '../../src/models/repository'
const repository = { path: '/repo-a' } as Repository

describe('Rust branch queries', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
  })

  it('uses bridge reads directly and propagates selection errors', async () => {
    mocks.transport = 'ws'
    mocks.merged.mockResolvedValueOnce([
      { ref: 'refs/heads/main', oid: 'a' },
      { ref: 'refs/heads/topic', oid: 'b' },
    ])
    await expect(getMergedBranches(repository, 'main')).resolves.toEqual(
      new Map([['refs/heads/topic', 'b']])
    )
    mocks.pointed.mockResolvedValueOnce(null).mockResolvedValueOnce(['main'])
    await expect(
      getBranchesPointedAt(repository, 'missing')
    ).resolves.toBeNull()
    await expect(getBranchesPointedAt(repository, 'HEAD')).resolves.toEqual([
      'main',
    ])
    const error = new Error('repository changed')
    mocks.pointed.mockRejectedValueOnce(error)
    await expect(getBranchesPointedAt(repository, 'HEAD')).rejects.toBe(error)
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('returns merged branches except the selected base under the read lease', async () => {
    mocks.merged.mockResolvedValue([
      { ref: 'refs/heads/main', oid: 'a' },
      { ref: 'refs/heads/topic', oid: 'b' },
    ])
    expect(await getMergedBranches(repository, 'main')).toEqual(
      new Map([['refs/heads/topic', 'b']])
    )
    expect(mocks.merged).toHaveBeenCalledWith('/repo-a', 'main')
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('keeps missing revisions distinct from a failed Rust request', async () => {
    mocks.pointed
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(['heads/topic'])
    await expect(
      getBranchesPointedAt(repository, 'missing')
    ).resolves.toBeNull()
    await expect(getBranchesPointedAt(repository, 'HEAD')).resolves.toEqual([
      'heads/topic',
    ])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates backend failures without falling back', async () => {
    const error = new Error('unavailable')
    mocks.merged.mockRejectedValue(error)
    await expect(getMergedBranches(repository, 'main')).rejects.toBe(error)
    mocks.pointed.mockRejectedValue(error)
    await expect(getBranchesPointedAt(repository, 'HEAD')).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('does not bypass a rejected repository lease', async () => {
    const error = new Error('ownership fence')
    mocks.scope.mockRejectedValueOnce(error)
    await expect(getMergedBranches(repository, 'main')).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
