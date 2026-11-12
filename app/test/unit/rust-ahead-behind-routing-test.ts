import { beforeEach, describe, expect, it, vi } from 'vitest'

const mocks = vi.hoisted(() => ({
  transport: 'wry',
  aheadBehind: vi.fn(),
  revisions: vi.fn(),
  hasMerge: vi.fn(),
  git: vi.fn(),
  scope: vi.fn(
    async (_path: string, _kind: string, run: () => Promise<unknown>) => run()
  ),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: mocks.transport,
    repositoryAheadBehind: mocks.aheadBehind,
    repositoryRevisionList: mocks.revisions,
    repositoryHasMergeCommits: mocks.hasMerge,
  }),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: mocks.scope,
}))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git }))

import {
  doMergeCommitsExistAfterCommit,
  getAheadBehind,
  getCommitsInRange,
} from '../../src/lib/git/rev-list'
import { GitError } from 'dugite'
import type { Repository } from '../../src/models/repository'
const repository = { path: '/repo-a' } as Repository

describe('Rust ahead/behind lookup', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
  })

  it('uses the bridge and preserves a missing revision without a shell fallback', async () => {
    mocks.transport = 'ws'
    mocks.aheadBehind
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce({ ahead: 2, behind: 1 })
    await expect(
      getAheadBehind(repository, 'missing...main')
    ).resolves.toBeNull()
    await expect(getAheadBehind(repository, 'topic...main')).resolves.toEqual({
      ahead: 2,
      behind: 1,
    })
    const error = new Error('repository changed')
    mocks.aheadBehind.mockRejectedValueOnce(error)
    await expect(getAheadBehind(repository, 'topic...main')).rejects.toBe(error)
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses the selected range under the repository read lease', async () => {
    mocks.aheadBehind.mockResolvedValue({ ahead: 3, behind: 2 })
    await expect(
      getAheadBehind(repository, 'local...origin/main')
    ).resolves.toEqual({ ahead: 3, behind: 2 })
    expect(mocks.aheadBehind).toHaveBeenCalledWith(
      '/repo-a',
      'local...origin/main'
    )
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves an empty result without repeating the query', async () => {
    mocks.aheadBehind.mockResolvedValue(null)
    await expect(getAheadBehind(repository, 'main...main')).resolves.toBeNull()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses existing Git behavior when the Rust read fails', async () => {
    mocks.aheadBehind.mockRejectedValue(new Error('unavailable'))
    mocks.git.mockResolvedValue({ stdout: '2\t4\n', gitError: null })
    await expect(getAheadBehind(repository, 'topic...main')).resolves.toEqual({
      ahead: 2,
      behind: 4,
    })
    expect(mocks.git).toHaveBeenCalledOnce()
  })

  it('propagates lease errors without retrying', async () => {
    const error = new Error('ownership fence')
    mocks.scope.mockRejectedValueOnce(error)
    await expect(getAheadBehind(repository, 'topic...main')).rejects.toBe(error)
    expect(mocks.git).not.toHaveBeenCalled()
  })
})

describe('Rust revision and merge history', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = 'wry'
  })

  it('routes revision and merge queries through the bridge without shell fallback', async () => {
    mocks.transport = 'ws'
    mocks.revisions
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(`${'a'.repeat(40)} first\n`)
    await expect(
      getCommitsInRange(repository, 'missing..main')
    ).resolves.toBeNull()
    await expect(getCommitsInRange(repository, 'base..main')).resolves.toEqual([
      { sha: 'a'.repeat(40), summary: 'first' },
    ])
    mocks.hasMerge.mockResolvedValueOnce(false)
    await expect(
      doMergeCommitsExistAfterCommit(repository, null)
    ).resolves.toBe(false)
    const error = new Error('repository changed')
    mocks.hasMerge.mockRejectedValueOnce(error)
    await expect(
      doMergeCommitsExistAfterCommit(repository, 'base')
    ).rejects.toBe(error)
    expect(mocks.scope).not.toHaveBeenCalled()
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('preserves rebase order and the existing one-line parser', async () => {
    const first = 'a'.repeat(40)
    const second = 'b'.repeat(40)
    mocks.revisions.mockResolvedValue(
      `${first} first summary\n${second} second summary\n${'c'.repeat(
        64
      )} ignored sha256\n`
    )
    await expect(getCommitsInRange(repository, 'base..topic')).resolves.toEqual(
      [
        { sha: first, summary: 'first summary' },
        { sha: second, summary: 'second summary' },
      ]
    )
    expect(mocks.revisions).toHaveBeenCalledWith('/repo-a', 'base..topic')
    expect(mocks.scope).toHaveBeenCalledWith(
      '/repo-a',
      'read',
      expect.any(Function),
      true
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('uses the legacy invalid revision result after a Rust error', async () => {
    mocks.revisions.mockRejectedValue(new Error('invalid revision'))
    mocks.git.mockResolvedValue({ gitError: GitError.BadRevision, stdout: '' })
    await expect(
      getCommitsInRange(repository, 'missing..topic')
    ).resolves.toBeNull()
  })

  it('uses the right revision for merge detection and preserves false', async () => {
    mocks.hasMerge.mockResolvedValueOnce(false).mockResolvedValueOnce(true)
    await expect(
      doMergeCommitsExistAfterCommit(repository, null)
    ).resolves.toBe(false)
    await expect(
      doMergeCommitsExistAfterCommit(repository, 'base')
    ).resolves.toBe(true)
    expect(mocks.hasMerge.mock.calls).toEqual([
      ['/repo-a', 'HEAD'],
      ['/repo-a', 'base..HEAD'],
    ])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('does not bypass the repository lease', async () => {
    const error = new Error('ownership fence')
    mocks.scope.mockRejectedValueOnce(error)
    await expect(getCommitsInRange(repository, 'base..topic')).rejects.toBe(
      error
    )
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
