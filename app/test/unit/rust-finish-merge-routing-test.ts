import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Repository } from '../../src/models/repository'
import { AppFileStatusKind, WorkingDirectoryFileChange } from '../../src/models/status'
import { DiffSelection, DiffSelectionType } from '../../src/models/diff'
import { ManualConflictResolution } from '../../src/models/manual-conflict-resolution'

const mocks = vi.hoisted(() => ({
  transport: 'wry', finish: vi.fn(), stage: vi.fn(), manual: vi.fn(), git: vi.fn(),
  scope: vi.fn(async (_path: string, _kind: string, work: () => Promise<unknown>) => work()),
}))
vi.mock('../../src/backend/client', () => ({ backend: () => ({ transportKind: mocks.transport, finishRepositoryMergeCommit: mocks.finish }) }))
vi.mock('../../src/lib/git/repository-operation', () => ({ withRepositoryGitOperation: mocks.scope }))
vi.mock('../../src/lib/git/core', () => ({ git: mocks.git, parseCommitSHA: () => 'legacy' }))
vi.mock('../../src/lib/git/update-index', () => ({ stageFiles: mocks.stage, preparePartialStagingDiffs: vi.fn() }))
vi.mock('../../src/lib/git/reset', () => ({ unstageAll: vi.fn() }))
vi.mock('../../src/lib/git/stage', () => ({ stageManualConflictResolution: mocks.manual }))

import { createMergeCommit } from '../../src/lib/git/commit'

const repository = { path: '/repo' } as Repository
const files = ['manual', 'edited'].map(path => new WorkingDirectoryFileChange(path, { kind: AppFileStatusKind.Modified }, DiffSelection.fromInitialSelection(DiffSelectionType.All)))

describe.each(['wry', 'ws'])('Rust merge completion through %s', transport => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.transport = transport
    mocks.finish.mockResolvedValue('[main abc1234] Merge\n')
  })

  it('resolves and stages conflicts before committing the existing index', async () => {
    const resolutions = new Map([['manual', ManualConflictResolution.ours]])
    await expect(createMergeCommit(repository, files, resolutions)).resolves.toBe('abc1234')
    expect(mocks.manual).toHaveBeenCalledWith(repository, files[0], ManualConflictResolution.ours)
    expect(mocks.stage).toHaveBeenCalledWith(repository, [files[1]])
    expect(mocks.finish).toHaveBeenCalledWith('/repo')
    expect(mocks.finish.mock.invocationCallOrder[0]).toBeGreaterThan(mocks.stage.mock.invocationCallOrder[0])
    expect(mocks.git).not.toHaveBeenCalled()
  })

  it('keeps the squash-abort commit and reset on the same legacy path', async () => {
    mocks.git.mockResolvedValue({ stdout: '[main abc1234] merge\n' })
    await expect(createMergeCommit(repository, [], new Map(), { useNative: false })).resolves.toBe('legacy')
    expect(mocks.finish).not.toHaveBeenCalled()
    expect(mocks.git).toHaveBeenCalledOnce()
  })

  it('does not fall back or replay after an uncertain outcome', async () => {
    mocks.finish.mockRejectedValue(new Error('inspect HEAD'))
    await expect(createMergeCommit(repository, [])).rejects.toThrow('inspect HEAD')
    expect(mocks.finish).toHaveBeenCalledOnce()
    expect(mocks.git).not.toHaveBeenCalled()
  })
})
