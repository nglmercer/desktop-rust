import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Branch } from '../../src/models/branch'
import type { Repository } from '../../src/models/repository'
import { ComputedAction } from '../../src/models/computed-action'

const calls = vi.hoisted(() => ({
  transport: 'ws',
  preview: vi.fn(),
  git: vi.fn(),
}))
vi.mock('../../src/backend/client', () => ({
  backend: () => ({
    transportKind: calls.transport,
    repositoryMergeTreePreview: calls.preview,
  }),
}))
vi.mock('../../src/lib/git/core', () => ({
  git: calls.git,
  isGitError: vi.fn(),
}))
vi.mock('../../src/lib/git/repository-operation', () => ({
  withRepositoryGitOperation: async (
    _path: string,
    _kind: string,
    action: () => Promise<unknown>
  ) => action(),
}))
import { determineMergeability } from '../../src/lib/git/merge-tree'

describe.each(['ws', 'wry'])('Rust merge preview via %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    calls.transport = transport
  })
  it('routes all merge outcomes and propagates errors', async () => {
    const repository = { path: '/repo' } as Repository
    const ours = { tip: { sha: 'a'.repeat(40) } } as Branch
    const theirs = { tip: { sha: 'b'.repeat(40) } } as Branch
    calls.preview
      .mockResolvedValueOnce({ kind: 'clean' })
      .mockResolvedValueOnce({ kind: 'conflicts', conflictedFiles: 2 })
      .mockResolvedValueOnce({ kind: 'invalid' })
      .mockRejectedValueOnce(new Error('merge failed'))
    await expect(
      determineMergeability(repository, ours, theirs)
    ).resolves.toEqual({ kind: ComputedAction.Clean })
    await expect(
      determineMergeability(repository, ours, theirs)
    ).resolves.toEqual({ kind: ComputedAction.Conflicts, conflictedFiles: 2 })
    await expect(
      determineMergeability(repository, ours, theirs)
    ).resolves.toEqual({ kind: ComputedAction.Invalid })
    await expect(
      determineMergeability(repository, ours, theirs)
    ).rejects.toThrow('merge failed')
    expect(calls.preview).toHaveBeenCalledWith(
      '/repo',
      ours.tip.sha,
      theirs.tip.sha
    )
    expect(calls.git).not.toHaveBeenCalled()
  })
})
