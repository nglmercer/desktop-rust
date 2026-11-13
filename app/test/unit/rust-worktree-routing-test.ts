import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  read: vi.fn(),
  mutate: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryWorktrees: mocks.read,
    mutateRepositoryWorktree: mocks.mutate,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))

import {
  listWorktrees,
  listWorktreesFromGitDir,
  resolveMainWorktreePath,
  addWorktree,
  removeWorktree,
  moveWorktree,
} from '../../src/lib/git/worktree'
import type { Repository } from '../../src/models/repository'

describe.each(['wry', 'ws'])('Rust worktree routing via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
  })

  it('reads the explicitly requested repository and preserves worktree fields', async () => {
    const entry = {
      path: '/repo/linked',
      head: 'abc',
      branch: null,
      isDetached: true,
      type: 'linked',
      isLocked: true,
      isPrunable: false,
    }
    mocks.read.mockResolvedValue([entry])
    await expect(listWorktrees('/repo')).resolves.toEqual([entry])
    expect(mocks.read).toHaveBeenCalledWith('/repo')
    expect(mocks.scope).toHaveBeenCalledTimes(transport === 'wry' ? 1 : 0)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('routes create, remove, and move through reviewed native operations', async () => {
    const repository = { path: '/repo' } as Repository
    mocks.mutate.mockResolvedValue(undefined)
    await addWorktree(repository, '/linked', {
      createBranch: 'topic',
      commitish: 'HEAD',
    })
    await removeWorktree('/repo', '/linked', true)
    await moveWorktree(repository, '/linked', '/moved')
    expect(mocks.mutate).toHaveBeenNthCalledWith(1, '/repo', {
      kind: 'add',
      path: '/linked',
      create_branch: 'topic',
      commitish: 'HEAD',
    })
    expect(mocks.mutate).toHaveBeenNthCalledWith(2, '/repo', {
      kind: 'remove',
      path: '/linked',
      force: true,
    })
    expect(mocks.mutate).toHaveBeenNthCalledWith(3, '/repo', {
      kind: 'move',
      from: '/linked',
      to: '/moved',
    })
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('propagates errors rather than invoking legacy Git', async () => {
    const error = new Error('unavailable')
    mocks.read.mockRejectedValue(error)
    await expect(listWorktrees('/repo')).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('recovers the main worktree using the missing worktree Git directory', async () => {
    const entry = {
      path: '/repo',
      head: 'abc',
      branch: 'refs/heads/main',
      isDetached: false,
      type: 'main',
      isLocked: false,
      isPrunable: false,
    }
    mocks.read.mockResolvedValue([entry])
    const gitDir = '/repo/.git/worktrees/missing'
    await expect(listWorktreesFromGitDir(gitDir)).resolves.toEqual([entry])
    const repository = { path: '/missing', gitDir } as Repository
    await expect(resolveMainWorktreePath(repository)).resolves.toBe('/repo')
    expect(mocks.read).toHaveBeenCalledWith(gitDir, true)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
